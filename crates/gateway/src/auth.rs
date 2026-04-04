use crate::{Context, Next, Plugin};
use futures::future::BoxFuture;
use http_server::Response as GatewayResponse;
use jsonwebtoken::{decode, DecodingKey, Validation, Algorithm};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Configuration for the JWT authentication plugin.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct AuthConfig {
    /// Shared secret for signature verification (HS256).
    pub secret: String,
    /// Configurable algorithm, defaults to HS256.
    pub algorithm: Option<String>,
    /// Optional expected issuer (`iss`).
    pub issuer: Option<String>,
    /// Optional expected audience (`aud`).
    pub audience: Option<String>,
    /// Paths that bypass authentication (e.g. ["/health", "/login"]).
    #[serde(default)]
    pub bypass_paths: Vec<String>,
}

/// Decoded claims from the JWT.
#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String,
    pub exp: usize,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

/// Authentication plugin for Phase 13: Gateway JWT Auth.
pub struct AuthPlugin {
    config: AuthConfig,
}

impl AuthPlugin {
    pub fn new(config: AuthConfig) -> Self {
        Self { config }
    }

    pub fn from_config(yaml: &str) -> anyhow::Result<Self> {
        let config: AuthConfig = serde_yaml::from_str(yaml)?;
        Ok(Self::new(config))
    }
}

impl Plugin for AuthPlugin {
    fn call<'a>(&'a self, ctx: &'a mut Context, next: Next) -> BoxFuture<'a, GatewayResponse> {
        Box::pin(async move {
            // 1. Check bypass list
            // Paths are checked for prefix matches to allow sub-paths on bypass.
            if self.config.bypass_paths.iter().any(|p| ctx.request.path.starts_with(p)) {
                tracing::info!("Bypassing auth for: {}", ctx.request.path);
                return next.run(ctx).await;
            }

            // 2. Extract Authorization header
            // Gateway keys are lowercased by the parser in http-server.
            let auth_header = match ctx.request.headers.get("authorization") {
                Some(h) => h,
                None => {
                    tracing::warn!("Auth missing for: {}", ctx.request.path);
                    return GatewayResponse::new(401);
                }
            };

            if !auth_header.starts_with("Bearer ") {
                tracing::warn!("Invalid auth header format");
                return GatewayResponse::new(401);
            }

            let token = &auth_header[7..];

            // 3. Verify signature and claims (exp is checked by default)
            let algorithm = match self.config.algorithm.as_deref().unwrap_or("HS256") {
                "HS256" => Algorithm::HS256,
                "HS384" => Algorithm::HS384,
                "HS512" => Algorithm::HS512,
                _ => {
                    tracing::error!("Unsupported JWT algorithm configured");
                    return GatewayResponse::new(401);
                }
            };

            let mut validation = Validation::new(algorithm);
            if let Some(iss) = &self.config.issuer {
                validation.set_issuer(&[iss]);
            }
            if let Some(aud) = &self.config.audience {
                validation.set_audience(&[aud]);
            }

            let key = DecodingKey::from_secret(self.config.secret.as_bytes());

            match decode::<Claims>(token, &key, &validation) {
                               Ok(token_data) => {
                    // 4. Attach status to Context metadata for downstream
                    ctx.metadata.insert("auth_sub".to_string(), token_data.claims.sub.clone());
                    
                    // Flattened additional claims
                    for (k, v) in &token_data.claims.extra {
                        let val_str = match v {
                            serde_json::Value::String(s) => s.clone(),
                            _ => v.to_string(),
                        };
                        ctx.metadata.insert(format!("auth_claim_{}", k), val_str);
                    }
                    
                    tracing::info!("JWT verified: sub={}", token_data.claims.sub);
                    next.run(ctx).await
                }

                Err(e) => {
                    tracing::warn!("JWT verification failed: {}", e);
                    GatewayResponse::new(401)
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Context, Next, Pipeline, Plugin};
    use http_server::{Request as GatewayRequest, Method, HttpVersion, Response as GatewayResponse};
    use jsonwebtoken::{encode, Header, EncodingKey};

    fn mock_context(path: &str, auth: Option<&str>) -> Context {
        let mut headers = HashMap::new();
        if let Some(token) = auth {
            headers.insert("authorization".to_string(), format!("Bearer {}", token));
        }
        let req = GatewayRequest {
            method: Method::GET,
            path: path.to_string(),
            version: HttpVersion::Http11,
            headers,
            params: HashMap::new(),
            body: vec![],
        };
        Context::new(req)
    }

    struct SuccessPlugin;
    impl Plugin for SuccessPlugin {
        fn call<'a>(&'a self, _ctx: &'a mut Context, _next: Next) -> BoxFuture<'a, GatewayResponse> {
            Box::pin(async { GatewayResponse::new(200) })
        }
    }

    #[tokio::test]
    async fn test_auth_plugin_valid_token() {
        let secret = "test_secret";
        let claims = Claims {
            sub: "user123".to_string(),
            exp: 9999999999, // far in the future
            extra: HashMap::new(),
        };
        let token = encode(&Header::default(), &claims, &EncodingKey::from_secret(secret.as_bytes())).unwrap();

        let config = AuthConfig {
            secret: secret.to_string(),
            algorithm: None,
            issuer: None,
            audience: None,
            bypass_paths: vec![],
        };
        let auth_plugin = AuthPlugin::new(config);
        let pipeline = Pipeline::new(vec![Box::new(auth_plugin), Box::new(SuccessPlugin)]);

        let ctx = mock_context("/api/data", Some(&token));
        let res = pipeline.execute(ctx).await;
        assert_eq!(res.status, 200);
    }

    #[tokio::test]
    async fn test_auth_plugin_expired_token() {
        let secret = "test_secret";
        let claims = Claims {
            sub: "user123".to_string(),
            exp: 1, // expired long ago
            extra: HashMap::new(),
        };
        let token = encode(&Header::default(), &claims, &EncodingKey::from_secret(secret.as_bytes())).unwrap();

        let config = AuthConfig {
            secret: secret.to_string(),
            algorithm: None,
            issuer: None,
            audience: None,
            bypass_paths: vec![],
        };
        let auth_plugin = AuthPlugin::new(config);
        let pipeline = Pipeline::new(vec![Box::new(auth_plugin), Box::new(SuccessPlugin)]);

        let ctx = mock_context("/api/data", Some(&token));
        let res = pipeline.execute(ctx).await;
        assert_eq!(res.status, 401);
    }

    #[tokio::test]
    async fn test_auth_plugin_wrong_algorithm() {
        let secret = "test_secret";
        let claims = Claims {
            sub: "user123".to_string(),
            exp: 9999999999,
            extra: HashMap::new(),
        };
        // Encode with HS512 but config defaults to HS256
        let header = Header::new(Algorithm::HS512);
        let token = encode(&header, &claims, &EncodingKey::from_secret(secret.as_bytes())).unwrap();

        let config = AuthConfig {
            secret: secret.to_string(),
            algorithm: Some("HS256".to_string()),
            issuer: None,
            audience: None,
            bypass_paths: vec![],
        };
        let auth_plugin = AuthPlugin::new(config);
        let pipeline = Pipeline::new(vec![Box::new(auth_plugin)]);

        let ctx = mock_context("/api/data", Some(&token));
        let res = pipeline.execute(ctx).await;
        assert_eq!(res.status, 401);
    }

    #[tokio::test]
    async fn test_auth_plugin_bypass() {
        let config = AuthConfig {
            secret: "secret".to_string(),
            algorithm: None,
            issuer: None,
            audience: None,
            bypass_paths: vec!["/health".to_string()],
        };
        let auth_plugin = AuthPlugin::new(config);
        let pipeline = Pipeline::new(vec![Box::new(auth_plugin), Box::new(SuccessPlugin)]);

        let ctx = mock_context("/health", None);
        let res = pipeline.execute(ctx).await;
        assert_eq!(res.status, 200);
    }
}
