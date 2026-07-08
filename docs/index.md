---
layout: home

hero:
  name: CKB Controller
  text: Game accounts & session keys on Nervos CKB
  tagline: Session-based game accounts — spend caps, policy gates, and gasless play on CKB and Fiber payment channels.
  actions:
    - theme: brand
      text: Get started
      link: /guide/
    - theme: alt
      text: Quickstart
      link: /guide/quickstart
    - theme: alt
      text: Internals
      link: /internals/

features:
  - title: Session keys with guardrails
    details: Per-session keys bounded by spend caps, expiry, and policy roots — sign gameplay without exposing the owner key.
  - title: Gasless play
    details: A paymaster sponsors fees behind a real biscuit-auth gate, so players transact without holding CKB.
  - title: Payment channels
    details: Fiber channels for high-frequency, off-chain pay-per-action that settle on-chain.
---
