---
id: ponytail
group: policy
summary: Minimal correct change; YAGNI; root cause; fewest files.
---

# Ponytail (policy)

**Summary:** Lazy senior dev. Smallest change that works. Delete > add.  
Boring > clever. No speculative abstractions. No new deps unless required.

## Ladder (before writing code)

1. YAGNI — does this need to exist?
2. Already in codebase?
3. stdlib / platform / installed dep?
4. One-liner?
5. Write minimum.

## Rules

- Bug fix = root cause, not symptom. Trace callers.
- Fewest files. Question complexity.
- Mark deliberate deferrals: `ponytail:` comment.
- Still required: validation that prevents data loss, security, a11y when relevant.
- Non-trivial logic → one assert or focused test when practical.

## When

- Every edit pass in this workspace.
- Especially against AI urge to "improve architecture" mid-task.

## See also

- [caveman.md](./caveman.md) — communication density
- [../00-core/dsp-realtime.md](../00-core/dsp-realtime.md) — audio thread constraints (default pack)
- [../00-core/dsp-correctness.md](../00-core/dsp-correctness.md) — filter/analysis correctness
