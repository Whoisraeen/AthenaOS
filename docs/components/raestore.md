# AthStore

The official app store. 12% revenue share. Sideloading allowed and supported.
No review hostage situations.

## Principles

- **12% take.** Below Apple's 15/30 and Google's 15/30, above Steam's 30 only because
  we're operating at smaller scale; revisit once shipped.
- **Sideloading is a feature.** `.rae` bundles install from any source, with a clear
  "unverified developer" prompt on first run — not a punitive scary one.
- **No hostage reviews.** Apps in the store get reviewed in a published SLA. If we
  miss it, the app gets to ship anyway with a notice.
- **No forced IAP system.** Apps may use AthStore IAP for the convenience, or their
  own payment system. Either way, 12%.

## Layers

- **athstore-client**: the user-facing app (built on AthUI, signed by us).
- **athstore-runtime**: install / update / verify on-device. Uses AthFS snapshots
  for atomic install and rollback.
- **athstore-backend** (out of scope here): the cloud-side catalog and review.

## Open design questions

- Subscription model (AthenaOS Pro etc.) inside or outside the store?
- Refund window — Steam's two-hour rule, or something gentler?
- Crypto/Web3-flavored app policy — banned, allowed-with-disclosure, or unrestricted?
