module.exports = {
  docs: [
    "intro",
    "getting-started",
    {
      type: "category",
      label: "API Reference",
      items: [
        "api/js-sdk",
        "api/http-server-package",
        "api/gateway-config",
      ],
    },
    {
      type: "category",
      label: "Guides",
      items: [
        "guides/coming-from-express",
        "guides/troubleshooting",
      ],
    },
    {
      type: "category",
      label: "Architecture",
      items: [
        "architecture/overview",
        "architecture/components",
        "architecture/ffi-bridge",
        "architecture/wal-format",
        "architecture/adr/rust-over-go",
        "architecture/adr/napi-bridge",
      ],
    },
  ],
};
