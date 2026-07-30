---
source: global
copied_by: template
date: 2026-07-29
adapted: false
reason: "biquad filter stability and implementation"
---

# Biquad Filter Design

**Summary:** Implementation rules for stable IIR biquads.
Emphasizes pole radius checks and numerically robust forms.
Prevents denormal slowdowns and trig overhead in hot loops.

- Stabilitätsprüfung: `|pole| < 1.0`
- Transposed Direct Form II bevorzugen
- Denormal-Schutz
- Keine trig-Funktionen im inneren Loop
