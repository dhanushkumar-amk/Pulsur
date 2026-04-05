const config = {
  title: "Pulsur Docs",
  tagline: "Rust-powered infrastructure for Node.js, documented for real shipping work.",
  favicon: "img/favicon.svg",
  url: "https://pulsur.dev",
  baseUrl: "/",
  organizationName: "pulsur",
  projectName: "pulsur",
  onBrokenLinks: "throw",
  markdown: {
    mermaid: true,
    hooks: {
      onBrokenMarkdownLinks: "throw",
    },
  },
  themes: ["@docusaurus/theme-mermaid"],
  i18n: {
    defaultLocale: "en",
    locales: ["en"],
  },
  presets: [
    [
      "classic",
      {
        docs: {
          routeBasePath: "/",
          sidebarPath: require.resolve("./sidebars.js"),
          editUrl: "https://github.com/pulsur/pulsur/tree/main/docs",
        },
        blog: false,
        theme: {
          customCss: require.resolve("./src/css/custom.css"),
        },
      },
    ],
  ],
  themeConfig: {
    image: "img/logo-mark.svg",
    colorMode: {
      defaultMode: "light",
      disableSwitch: false,
      respectPrefersColorScheme: false,
    },
    navbar: {
      title: "Pulsur",
      logo: {
        alt: "Pulsur",
        src: "img/logo-mark.svg",
      },
      items: [
        { type: "docSidebar", sidebarId: "docs", position: "left", label: "Docs" },
        { href: "https://github.com/pulsur/pulsur", label: "GitHub", position: "right" },
      ],
    },
    footer: {
      style: "dark",
      links: [
        {
          title: "Docs",
          items: [
            { label: "Getting Started", to: "/getting-started" },
            { label: "API Reference", to: "/api/js-sdk" },
            { label: "Architecture", to: "/architecture/overview" },
          ],
        },
        {
          title: "Project",
          items: [
            { label: "Benchmarks", href: "https://github.com/pulsur/pulsur/blob/main/BENCHMARKS.md" },
            { label: "Releasing", href: "https://github.com/pulsur/pulsur/blob/main/RELEASING.md" },
          ],
        },
      ],
      copyright: `Copyright ${new Date().getFullYear()} Pulsur.`,
    },
    prism: {
      additionalLanguages: ["rust", "toml", "bash", "yaml"],
    },
  },
};

module.exports = config;
