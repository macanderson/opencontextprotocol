# Context reuse: identity, accounting, consent, and verification

This is a companion to the [protocol surface](./protocol-surface.md). Where
that page defines the wire shapes for a *single* retrieval, this one defines
the four interlocking guarantees that make **reusing** retrieved context
across turns safe, cheap, and auditable:

1. **[Deterministic composition](#1-deterministic-composition)** — a stable
   frame identity and a canonical ordering, so an unchanged context set renders
   byte-identically and stays friendly to provider prompt caches.
2. **[Usage reports](#2-usage-reports)** — a per-request roll-up of frame costs
   an auditor can walk from an invoice line back to the exact frames.
3. **[Consent scopes and receipts](#3-consent-scopes-and-receipts)** — a closed
   vocabulary of egress classes and a durable receipt, so "did it leave the
   machine, to whom, and who agreed?" is answerable years later.
4. **[Context verification](#4-context-verification)** — a pull-based
   `context/verify` request that revalidates held frames cheaply, without
   re-sending any frame body.

They share one spine: the **frame identity** defined in §1. A usage report
references served frames by it; a verify request carries it. Build order and
dependency run top to bottom.

> **Conformance language.** **MUST**, **MUST NOT**, **SHOULD**, **MAY** in
> **bold** follow [BCP 14 / RFC 2119](https://www.rfc-editor.org/rfc/rfc2119),
> exactly as in [protocol-surface.md](./protocol-surface.md). Each section ends
> with its own conformance requirements; they are also consolidated into the
> [protocol-surface conformance table](./protocol-surface.md#conformance-requirements).

> **Spec-version implications.** Everything here is **additive** within the
> `contextgraph/1` family: new optional fields (`content_digest`,
> `egress_scopes`), new capability-gated methods (`verify`), and new host-side
> artifacts (`UsageReport`, `ConsentReceipt`) that ride no new required wire
> field. A `contextgraph/1.0-draft` provider that implements none of them still
> handshakes and answers queries; a host that wants them degrades to its
> existing behavior (re-query, boolean consent) when a provider opts out. No
> flag day — see [stability.md](./stability.md).

---

## 1. Deterministic composition

### The problem

Provider prompt caches reward a **byte-stable prompt prefix**: Anthropic bills
cache reads at 0.1× input, OpenAI and Gemini cache long prefixes automatically.
Context retrieval is the part of a prompt most likely to destroy that
stability. A host that re-queries its providers each turn and concatenates
frames in arrival order produces a *different* prefix every turn — silently
forfeiting the cache and multiplying the very token costs this protocol exists
to make honest. And nothing today stops two conformant hosts, given the
identical frames, from rendering arbitrarily different prompts.

This is distinct from the reference composition module (budget packing, dedup,
injection-resistance — issue #15). This section is the narrower **contract**
that any composition, reference or not, can satisfy to be cache-friendly.

### Frame identity

A frame's exact bytes are identified by the triple **`(provider id, frame id,
content digest)`**, bound to Rust as
[`FrameId`](https://docs.rs/contextgraph-types/latest/contextgraph_types/identity/struct.FrameId.html):

```rust
pub struct FrameId {
    pub provider_id: String,           // the host's routing/consent key for the serving provider
    pub frame_id: String,              // ContextFrame::id — provider-scoped, stable for dedup
    pub content_digest: Option<String>,// provider-declared, opaque (e.g. "sha256:<hex>")
}
```

- **The `content_digest` is provider-declared and opaque.** A provider chooses
  the algorithm; the reference frames use `sha256:<hex>`, matching the
  `provenance` digests. The protocol never re-derives it. This is deliberate:
  a host that computed the digest from its *own* canonical serialization would
  force every non-Rust provider to byte-exactly reproduce that serialization
  just to answer a [verify](#4-context-verification) request — precisely the
  lock-in the protocol avoids. The cost is that identity is a provider
  *promise* rather than a host-checkable fact; §4's conformance case is what
  holds a provider to that promise.
- A frame **MAY** omit its digest (`content_digest: None` /
  [`ContextFrame::content_digest`](./protocol-surface.md#context-frame) absent).
  Such a frame is **not verifiable**: a host **MUST** treat it as un-revalidatable
  and re-query rather than reuse it unchecked (§4).

Two frames with the same identity **MUST** have the same content bytes;
changing a frame's content **MUST** change its `content_digest` (else §4's
staleness detection cannot work).

### Canonical ordering

The **canonical composition order** is the total order over `FrameId` given by
comparing `provider_id`, then `frame_id`, then `content_digest`
(`Option<String>`, `None` before `Some`). A host that composes a set of frames
into a single context block:

- **MUST** emit them in canonical order, so an unchanged frame set renders
  byte-identically across turns *and* across hosts;
- **MUST NOT** let a frame's `score` or `token_cost` affect the rendered bytes
  — relevance is query-dependent and cost is derived, so a re-query that only
  re-ranks the same frames would otherwise bust the prefix for no content
  change;
- **SHOULD** de-duplicate identical identities to a single rendered block.

The reference host implements this in
[`compose_context`](https://docs.rs/contextgraph-host/latest/contextgraph_host/compose/fn.compose_context.html)
(and [`FanOut::compose`](https://docs.rs/contextgraph-host/latest/contextgraph_host/host/struct.FanOut.html#method.compose)).
Frame `content` is emitted inside an explicit `<frame>…</frame>` fence as
quoted material, never as instructions (protocol-surface R3).

### Prefix stability (informative)

Canonical ordering guarantees cross-turn and cross-host determinism. To
*maximize* cache hits, a host **SHOULD** additionally prefer **append-only**
composition: place newly-retrieved frames after the frames retained from the
previous turn, so a turn that only *adds* context extends the cached prefix
instead of reordering it. This is guidance, not a conformance requirement — a
host that re-sorts the whole set each turn is still conformant, just less
cache-efficient on turns that add high-sorting frames.

### Rationale: the cache economics (appendix)

Why bake this into the protocol rather than leave it to each host? Because the
failure is silent and expensive, and it compounds exactly where this protocol
claims to help. Consider a 20-turn agent session with an 8-frame, ~4,000-token
context block:

- **Arrival-order composition.** Each turn's retrieval returns the same frames
  in a slightly different order (vector search is not order-stable), so the
  prompt prefix changes every turn. Every turn pays full input price on the
  whole context block: `20 × 4,000 = 80,000` context tokens billed at 1×.
- **Canonical-order composition.** The context block is byte-identical on turns
  that don't change the underlying frames. Turn 1 pays 1× to write the cache;
  turns 2–20 read it at 0.1×: `4,000 + 19 × 4,000 × 0.1 = 11,600` tokens. A
  **~7× reduction** on the context portion of the prompt — with no change to
  what the model sees.

The saving is not hypothetical head-room; it is the difference between a
protocol that *measures* context cost honestly (budget honesty, §protocol-surface
B1) and one that also *lets a host control* it. Determinism is the small,
mechanical precondition that turns "we can see the cost" into "we can cut the
cost," and pairing it with [verification](#4-context-verification) is what makes
reuse both cache-friendly *and* safe from serving stale evidence.

### Conformance (§1)

| # | Requirement | Enforced / verified by |
| - | ----------- | ---------------------- |
| D1 | Frames with the same `FrameId` **MUST** have identical content bytes; changing content **MUST** change `content_digest`. | provider contract; `verify` conformance case (§4) |
| D2 | A host composing a frame set **MUST** emit frames in canonical `FrameId` order, independent of arrival order. | `compose_context` round-trip test |
| D3 | Composition **MUST NOT** depend on `score` or `token_cost`. | `compose_context` relevance-invariance test |
| D4 | A frame with no `content_digest` **MUST** be treated as unverifiable and re-queried, never reused unchecked. | host verify fallback (§4) |
