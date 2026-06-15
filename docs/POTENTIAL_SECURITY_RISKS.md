# Potential Security Risks

A living checklist of identified security risks. Items are unchecked until a mitigation is implemented and verified.

---

## Tunnel Lifecycle

- [ ] **Dangling CF tunnel pointing to unguarded local port** — If the gateway exits without cleanly stopping the Cloudflare tunnel, the tunnel remains active. Any other process that subsequently binds to the same local port would receive public internet traffic with no authentication enforced. Mitigation: PID file + explicit kill on shutdown + OS-level pdeathsig (Linux).

---

## Binary Integrity

- [ ] **Tampered release binaries** — A compromised S3 bucket, CDN, or GitHub release asset could serve a modified binary to users. Many open-source projects publish per-release checksums (SHA-256) and GPG/sigstore signatures alongside the binary so users can verify authenticity before running. We should do the same.

---

## Container Port Exposure (macOS)

- [ ] **Mapped container port accessible to any local process on macOS** — When running the MCP container with a host port mapping on macOS (native containers or Docker Desktop), it is not yet confirmed whether the bound port is accessible to *any* process on the local machine or only to the gateway process. If it is broadly accessible, a local process could bypass OAuth entirely by talking directly to the MCP upstream. Needs investigation.
