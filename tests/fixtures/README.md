# Generator Fixtures

This directory defines the canonical layout for `onc-rpcgen` fixtures.

## Layout

- `xdr/real/`
  Real `.x` files copied or mirrored from active consumer schemas.
- `xdr/synthetic/`
  Minimal `.x` files for isolated generator edge cases.
- `expected/`
  Snapshot outputs or other expected generator artifacts.

## Intent

Prefer real `.x` inputs whenever they cover the behavior being added or fixed.
Use synthetic fixtures only when a small isolated schema is the clearest way to
exercise a specific parser or generator path.
