# Rust Agentic Programming
## Building AI Agents for Java Developers

**A practical guide to building production-ready AI agents in Rust — written for engineers migrating from Java and the Spring AI / LangChain4j ecosystem**

---

### About This Book

This book teaches experienced Java developers how to build AI agents in Rust using the four key frameworks in the Rust AI ecosystem:

- **rig-core** — LLM clients, tool calling, structured output, RAG, and multi-turn agents
- **swiftide** — Streaming document indexing and RAG query pipelines
- **graph-flow** — Stateful multi-step agent workflows as directed graphs with session persistence
- **rmcp** — Model Context Protocol (MCP) server and client implementation

Every chapter includes a Java parallel — a direct comparison showing the equivalent Spring AI, LangChain4j, or LangGraph4j pattern, so you can map what you already know to idiomatic Rust.

---

### Who This Book Is For

This book is written for **experienced Java developers** who:
- Are proficient in Java (11+, ideally 17+) and the Spring ecosystem
- Want to understand Rust's value proposition for AI agent development
- Are building production systems where performance, memory safety, and binary size matter
- Want to learn Rust through a lens of practical AI application development

No prior Rust knowledge required — Chapter 2 covers Rust fundamentals for Java developers.

---

### What You'll Learn

By the end of this book, you will be able to:

1. Use Rust's ownership model to build memory-safe AI agents with zero garbage collection pauses
2. Integrate with LLM APIs using `rig-core` for tool calling, structured output, and multi-turn conversations
3. Build streaming RAG pipelines with `swiftide` — chunk, embed, and query document corpora
4. Design stateful multi-agent workflows as directed graphs with `graph-flow`
5. Expose and consume MCP tools using `rmcp` for cross-language agent interoperability
6. Implement human-in-the-loop approval gates with session persistence
7. Add observability (tracing, metrics, structured logging) and security guards to production agents
8. Deploy agentic Rust services as Docker containers with multi-stage builds

---

### Book Structure

| Part | Title | Chapters |
|------|-------|----------|
| I | **Foundations** | 1–3 |
| II | **Core Rig Capabilities** | 4–7 |
| III | **RAG and Memory** | 8–11 |
| IV | **Orchestration with graph-flow** | 12–15 |
| V | **Production** | 16–21 |

**Part I — Foundations**
- Ch 1: Why Rust for Agentic AI?
- Ch 2: Rust for Java Developers
- Ch 3: LLM Basics in Rust

**Part II — Core Rig Capabilities**
- Ch 4: Tool Calling with rig-core
- Ch 5: Structured Output
- Ch 6: Agents and Multi-Turn Conversations
- Ch 7: Building a Streaming API with Axum

**Part III — RAG and Memory**
- Ch 8: RAG with rig-core
- Ch 9: Swiftide — Streaming Indexing Pipelines
- Ch 10: Memory and State
- Ch 11: MCP — Model Context Protocol

**Part IV — Orchestration**
- Ch 12: Graph-Based Workflows
- Ch 13: Graph Agents
- Ch 14: Stateful Workflows and Persistence
- Ch 15: Multi-Agent Pipelines

**Part V — Production**
- Ch 16: Observability, Security, and Cost Control
- Ch 17: Deployment — The Rust Advantage
- Ch 18: Framework Comparison
- Ch 19: Capstone — Research Agent
- Ch 20: Capstone — Multi-Agent Pipeline
- Ch 21: Production-Ready Agent

---

### Prerequisites

- Java 11+ familiarity (the book draws Java parallels throughout)
- Rust toolchain: `rustup` + `cargo` ([install](https://rustup.rs))
- An OpenAI API key (set as `OPENAI_API_KEY`)
- Docker (for deployment chapter)

---

### Running the Examples

```bash
git clone https://github.com/jackandjac/Book-Rust-Agentic-Programming.git
cd Book-Rust-Agentic-Programming/code-examples

export OPENAI_API_KEY="sk-..."
cargo run -p ch04-tool-calling
cargo run -p ch09-swiftide
cargo run -p ch14-stateful-workflows
RUST_LOG=info cargo run -p ch19-capstone-research-agent
```

Each crate matches its chapter number. See the chapter's prose for full setup instructions.

---

### Generating the PDF

```bash
# Requires Node.js
npm install
npx puppeteer browsers install chrome
./generate-pdf.sh

# Force regenerate
./generate-pdf.sh --force
```

The generated `COMPLETE_BOOK.pdf` contains all 21 chapters with syntax-highlighted code blocks.

---

### Framework Versions

| Framework | Version | Downloads |
|-----------|---------|-----------|
| rig-core | 0.37 | 772k |
| swiftide | 0.32 | 81k |
| graph-flow | 0.5.1 | 6.6k |
| rmcp | 1.6 | 9.7M |
| axum | 0.8 | 74M |
| tokio | 1 | 290M |

---

**Version**: 1.0 (May 2026)

---

© 2026 | Licensed under Creative Commons Attribution-NonCommercial 4.0
