# Potential Security Risks

A living checklist of identified security risks. Items are unchecked until a mitigation is implemented and verified.

---

## Binary Integrity

- [ ] **Tampered release binaries** — A compromised S3 bucket, CDN, or GitHub release asset could serve a modified binary to users. Many open-source projects publish per-release checksums (SHA-256) and GPG/sigstore signatures alongside the binary so users can verify authenticity before running. We should do the same.

---

## Container Port Exposure (macOS)

- [ ] **Mapped container port accessible to any local process on macOS** — When running the MCP container with a host port mapping on macOS (native containers or Docker Desktop), it is not yet confirmed whether the bound port is accessible to *any* process on the local machine or only to the gateway process. If it is broadly accessible, a local process could bypass OAuth entirely by talking directly to the MCP upstream. Needs investigation.


## Use stronger generated passwords

- [ ] **Weakly generated passwords** — the passwords generated in the setup do not contain symbols or uppercase letters, making them easier to guess. 

---

## Host Process Hardening

- [ ] **No process-level sandboxing for the Rust gateway** — the host process has ambient access to the whole filesystem and network; a compromised dependency or an exploited bug has no fs/network jail to contain it, unlike the MCP container which has neither outbound network access nor filesystem access beyond its bind mounts. Filesystem restriction (chroot/Landlock on Linux, sandbox-exec/App Sandbox on macOS) is a plausible future mitigation; network egress restriction is harder for a process that legitimately needs outbound access (`cloudflared`, container runtime API) but is still worth scoping later. See `docs/SECURITY_AUDIT_LATEST.md` finding 6.3.

---

## Custom Protocol Implementation Risk

- [ ] **Hand-rolled OAuth2.1 server implementation** — Brain3 implements OAuth2.1 itself rather than using a battle-tested server-side library. Rust's memory safety prevents whole classes of bugs (buffer overflows, use-after-free, etc.) but does not prevent protocol/logic-level vulnerabilities specific to this implementation (e.g. auth bypass, state confusion) that a maintained, widely-audited library might already have caught. See `docs/SECURITY_AUDIT_LATEST.md` finding 6.4.
