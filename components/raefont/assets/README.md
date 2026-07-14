# raefont bundled font assets

Embedded (via `include_bytes!`) so the kernel + apps have a real crisp font with
no filesystem dependency. Both are **SIL Open Font License 1.1** (embeddable;
license text ships alongside, per OFL §). See `docs/design/typography-rendering.md`.

| File | Role | Family | License |
|---|---|---|---|
| `Inter-Variable.ttf` | RaeSans — the humanist UI face (Inter[opsz,wght] variable; render the default/SemiBold instance) | sans | `Inter-OFL.txt` |
| `JetBrainsMono-Regular.ttf` | RaeMono — terminal / code | mono | `JetBrainsMono-OFL.txt` |

Source: Inter from `google/fonts` (`ofl/inter/`), JetBrains Mono from the official
`JetBrains/JetBrainsMono` v2.304 release. Do NOT rename the internal `name` table
(OFL Reserved Font Name). The §6 type ramp (`rae_tokens::TYPE_*`) selects size/weight.
