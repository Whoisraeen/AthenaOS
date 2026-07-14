# AthenaOS Safety — AthGuard

AthGuard is the safety and capability plane of AthenaOS. It dominates AthMind goals and AthBody commands.

## Hard rules

1. **Physical kill switch wins.** A hardware E-stop (or equivalent GPIO) cuts actuation power / command path regardless of software state.
2. **Capabilities gate the body.** Motor, mic, camera, network, and model-tool use require explicit caps.
3. **No silent safety self-mod.** AthMind cannot rewrite AthGuard policy, E-stop behavior, or joint limits without an owner-attested update path.
4. **Fail closed on actuators.** Cap check failure or watchdog timeout → safe pose / freeze, not “best effort.”
5. **Attestation for trust.** Boot and critical policy blobs are measurable; owner tools can verify.

## Surfaces

| Surface | Guard behavior |
|---|---|
| AthBody motor cmds | Rate limits, torque/velocity caps, workspace geofence |
| AthSense streams | Privacy caps (mic/camera); retention policy |
| AthMind tools / LLM | Network and code-exec caps; proposal-only to actuators |
| AthVoice | Consent for recording; volume/emotion expression limits |
| Updates | Signed images; rollback via AthFS snapshots |

## Mapping to inherited code

- Product face: `components/athguard/`
- Capability engine today: `components/athshield/` (AthGuard lineage)
- Kernel enforcement: capability checks on privileged syscalls (inherited AthKernel / AthKernel path)

## Consent and ownership

- Local-first: cloud is optional.
- Owner sets standing policy; AthenaOS does not ship ads or forced telemetry in the mind loop.
- Social presence (AthVoice) must respect recording and interaction consent flags.

## Incident model

| Event | Response |
|---|---|
| E-stop asserted | Immediate halt; log; require clear + policy check to resume |
| Cap violation | Deny; audit log; optional degrade autonomy level |
| Watchdog miss (control RT) | Freeze actuators; raise fault to AthMind / operator |
| Policy update | Require signature + reboot or attested hot-reload |

Details of syscall numbers and attestation APIs remain in inherited docs until the Athena rename pass rewrites them under Ath* names.
