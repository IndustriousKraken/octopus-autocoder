---
title: Investigate canonical-RAG re-embedding on every daemon restart
status: proposed
added: 2026-07-13
---

The canonical-spec RAG store (`autocoder/src/rag/`) is in-memory only. Every
daemon restart re-chunks and re-embeds all canonical specs via the configured
embedding provider (`rag/embedding.rs` — Ollama or an OpenAI-compatible API),
paying startup latency and, for hosted providers, per-token cost each time.

Probably accidental rather than a deliberate trade-off. Investigate whether a
disk cache keyed by content hash (spec chunk → embedding vector) is worth it,
or whether spec corpora stay small enough that re-embedding is fine — in which
case document that as the intended behavior and close this.

Origin: noticed during a codebase review, 2026-07-13. Not urgent.
