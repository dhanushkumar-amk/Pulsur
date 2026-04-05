import React from "react";
import Layout from "@theme/Layout";
import Link from "@docusaurus/Link";

const cards = [
  {
    title: "Start Fast",
    text: "Install, boot a local HTTP server, and confirm a working response in under five minutes.",
    to: "/getting-started",
  },
  {
    title: "Use The SDK",
    text: "See the current JS API surface for the HTTP server, queue, and rate limiter bridge.",
    to: "/api/js-sdk",
  },
  {
    title: "Understand The Internals",
    text: "Follow request flow, WAL persistence, and the napi-rs bridge from Node.js into Rust.",
    to: "/architecture/overview",
  },
];

export default function Home() {
  return (
    <Layout
      title="Pulsur Docs"
      description="Operational documentation for Pulsur infrastructure components."
    >
      <main className="hero-shell">
        <section className="hero-panel">
          <p className="eyebrow">Rust infrastructure for Node.js teams</p>
          <h1>Ship the fast path without guessing how it works.</h1>
          <p className="hero-copy">
            Pulsur packages HTTP serving, routing, rate limiting, queueing, and observability
            into one Rust-first toolkit with a pragmatic Node.js bridge.
          </p>
          <div className="hero-actions">
            <Link className="button button--primary button--lg" to="/getting-started">
              Get Started
            </Link>
            <Link className="button button--secondary button--lg" to="/architecture/overview">
              See Architecture
            </Link>
          </div>
        </section>

        <section className="card-grid">
          {cards.map((card) => (
            <Link key={card.title} className="doc-card" to={card.to}>
              <h2>{card.title}</h2>
              <p>{card.text}</p>
            </Link>
          ))}
        </section>
      </main>
    </Layout>
  );
}
