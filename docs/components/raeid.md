# RaeID

Account system. Passkeys first. Optional. **Never required for local use.**

## Principles

- The OS works fully without a RaeID account. Local user, local data, local apps,
  local games. Period.
- Sign-in is the on-ramp to RaeSync, RaeStore purchases tied to a user, and
  cross-device profile portability. None of those are mandatory.
- Passkeys (FIDO2 / WebAuthn) are the default. No passwords by default; passwords
  remain as a fallback only.
- No surveillance: see the EULA — no ads, no data sales, ever.

## Surface

```
1. First-boot: "Skip sign-in" is visually equal to "Sign in".
2. Account creation: passkey on this device, recovery via a second device or
   recovery code.
3. Lost device: recovery via passkey on a second device, or a printed recovery
   code held offline.
4. Federated sign-in: optional, for users who want Apple ID / Google linkage.
```

## Open design questions

- Backup of the local-only "no RaeID" user data — encouraged? automatic to encrypted
  external drive? out of scope?
- Family sharing model when accounts are present
- Recovery code length / encoding (BIP-39-style? raw base32?)
