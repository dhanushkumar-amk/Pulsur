#[cfg(not(feature = "noop"))]
fn main() {
    napi_build::setup();
}

#[cfg(feature = "noop")]
fn main() {}
