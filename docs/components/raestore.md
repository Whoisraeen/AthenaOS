# RaeStore

The official app store. 12% revenue share. Sideloading allowed and supported.
No review hostage situations.

## Principles

- **12% take.** Below Apple's 15/30 and Google's 15/30, above Steam's 30 only because
  we're operating at smaller scale; revisit once shipped.
- **Sideloading is a feature.** `.rae` bundles install from any source, with a clear
  "unverified developer" prompt on first run — not a punitive scary one.
- **No hostage reviews.** Apps in the store get reviewed in a published SLA. If we
  miss it, the app gets to ship anyway with a notice.
- **No forced IAP system.** Apps may use RaeStore IAP for the convenience, or their
  own payment system. Either way, 12%.

## Layers

- **raestore-client**: the user-facing app (built on RaeUI, signed by us).
- **raestore-runtime**: install / update / verify on-device. Uses RaeFS snapshots
  for atomic install and rollback.
- **raestore-backend** (out of scope here): the cloud-side catalog and review.

## Open design questions

- Subscription model (RaeenOS Pro etc.) inside or outside the store?
- Refund window — Steam's two-hour rule, or something gentler?
- Crypto/Web3-flavored app policy — banned, allowed-with-disclosure, or unrestricted?
