---
id: caveman
group: policy
summary: Dense agent communication; cut filler, keep technical precision.
---

# Caveman (policy)

**Summary:** Talk terse. Drop articles/filler/pleasantries. Fragments OK.  
Technical terms exact. Saves tokens; keeps diffs and answers scannable.

## Rules

- Prefer short clauses over essays.
- No ceremonial openers ("Great question", "I'd be happy to").
- Code, identifiers, paths, commands: exact.
- Security warnings and irreversible actions: normal clear prose.
- User can set intensity: lite | full | ultra (default full if active).

## When

- Default for agent replies during implementation.
- Not for user-facing product docs unless asked.

## See also

- [ponytail.md](./ponytail.md) — how to change code
- [../06-agents/agent-usage.md](../06-agents/agent-usage.md) — map reading order
