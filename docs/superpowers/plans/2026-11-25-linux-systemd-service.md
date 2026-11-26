# Start Brain3 at boot with systemd

## Goal

After one-time interactive setup, a Linux administrator can run Brain3 and
Cloudflare Tunnel as separate systemd services that start at boot, before the
owner logs in. Brain3 serves the existing vault and gateway on loopback.
`cloudflared.service` connects the existing public hostname to that gateway.
The vault, authentication credentials, token database, and DNS route carry
over from an existing installation.

## 1. Write the Linux setup guide in the README

- Add **Start at boot on Linux** to [README.MD](../../../README.MD) after
  interactive setup and link to it from **Advanced Configuration Options**,
  where the README currently mentions tmux. The repository has no existing
  service example. Put the complete, copy-pasteable unit in the README so
  users of the signed binary can follow the guide without a source checkout.
- Link to the existing Linux instructions for installing Docker and
  `cloudflared` from packages. Document one-time Brain3 setup as the future
  service user. Show how to locate Brain3 and verify that the account can
  reach Docker and access the vault and app config with its existing
  permissions. Explain the privileges of membership in the `docker` group.
  [Docker permissions on Linux](https://docs.docker.com/engine/install/linux-postinstall/#manage-docker-as-a-non-root-user)
- Put the Brain3 config for a tunnel run by systemd in a README table. Use an access mode
  that enables the OAuth gateway, set `B3_CF_QUICK_TUNNEL=false`, and clear
  `B3_CF_TUNNEL_NAME`, `B3_CF_DOMAIN`, and `B3_CF_TUNNEL_CONFIG_FILE`. Set
  `B3_DIRECT_PUBLIC_ORIGIN_HOSTNAME` to the tunnel's public hostname without a
  scheme or path. Keep hostname enforcement enabled and the gateway bound
  to loopback. Update the configuration reference to explain that the
  public hostname also applies to an externally managed tunnel.
- Check for an existing `cloudflared.service` before staging any files. If it
  serves a different tunnel, use a dedicated unit with its own config path.
  Use Cloudflare's local tunnel setup guide for a new named tunnel. For an
  existing local tunnel, reuse its configuration, tunnel credentials, and
  DNS route. Show how to place the service config and credentials under
  `/etc/cloudflared` with ownership and permissions appropriate to the
  service account, using an absolute `credentials-file` path. Point ingress at
  `http://127.0.0.1:<B3_OAUTH2_GATEWAY_PORT>` and route the whole hostname so
  OAuth discovery and authorization work alongside MCP. Validate ingress
  with the intended config path. [Cloudflare named tunnel setup](https://developers.cloudflare.com/tunnel/features/locally-managed-tunnels/create-local-tunnel/)
  · [Ingress configuration and validation](https://developers.cloudflare.com/tunnel/features/locally-managed-tunnels/configuration-file/)
- Show which fields to customize and how to save the example with
  `sudoedit /etc/systemd/system/brain3.service`. Walk through
  `sudo systemd-analyze verify /etc/systemd/system/brain3.service`,
  `sudo systemctl daemon-reload`, `sudo systemctl start brain3`, a loopback
  OAuth resource metadata probe with the public `Host` header, and
  `sudo systemctl enable brain3` in that order.
  Explain that an active unit means the binary launched, while the MCP check
  confirms startup completed.
- Follow Cloudflare's Linux instructions to install `cloudflared.service`
  with the explicit config path. Start Brain3 before running the tunnel
  installer, which can start and enable the tunnel immediately. Check that
  both services are active and enabled. If `cloudflared.service` already
  serves the tunnel, verify its route and reuse the existing service. If
  the unit serves a different tunnel, preserve it and use a separate unit
  for the Brain3 tunnel as needed. [Cloudflare's systemd instructions](https://developers.cloudflare.com/tunnel/features/locally-managed-tunnels/as-a-service/linux/)
  · [Linux service installer](https://github.com/cloudflare/cloudflared/blob/master/cmd/cloudflared/linux_service.go)
- Include status and current-boot journal commands for both services.
  Show how to restart and disable Brain3 and how to manage a dedicated
  `cloudflared.service` without interrupting a shared tunnel. Cover upgrades
  using the signed installer at the existing executable path followed by a
  service restart. For changes that need the TUI, stop the service, run the
  wizard as the service user, quit it cleanly, and restart the unit.

## 2. Define the unit in the README example

- Run `brain3 --cli` via an absolute `ExecStart` under `User=` for the existing
  Brain3 account. Use `Type=exec` with `Type=simple` as a compatibility fallback,
  an absolute `WorkingDirectory`, and set
  `HOME` and a standard system `PATH` with `Environment=`. The current CLI
  dependency check expects Docker and `cloudflared` on `PATH` even when
  systemd starts the tunnel. The process runs in the foreground and sends its
  logs to the journal with `StandardOutput=journal` and `StandardError=journal`.
  [systemd execution
  environment](https://www.freedesktop.org/software/systemd/man/latest/systemd.exec.html)
- Pass the configured Brain3 home with `--brain3-home`. Brain3 reads its own
  `.env` from that directory. Show `--env-file /etc/brain3/brain3.env` as a
  custom config path with mode `0600` and the service account as owner. Keep
  secrets in the Brain3 config and tunnel credential files.
- Require and order the unit after `docker.service`. Use
  `Wants=network-online.target` and `After=network-online.target` for boot-time
  image pulls. Enable both services independently so the gateway can start
  when Cloudflare is unavailable. Bind Brain3 enablement to
  `multi-user.target`. Document `RequiresMountsFor=` for a vault or app home
  on a separate mount and DNS
  troubleshooting if the network is slow at boot. [systemd unit
  dependencies](https://www.freedesktop.org/software/systemd/man/latest/systemd.unit.html)
  · [Network readiness](https://systemd.io/NETWORK_ONLINE/)
- Set `KillSignal=SIGINT` because the CLI currently waits for
  `tokio::signal::ctrl_c()` before draining and calling
  `shutdown_managed_runtime()`. Use `KillMode=mixed` so Brain3 receives the
  graceful stop signal before systemd cleans up any remaining processes in
  the unit. Set `TimeoutStopSec=60s` initially and verify it covers shutdown.
  Docker containers are managed by the daemon, so verify that Brain3
  removes the containers it started. [systemd process
  termination](https://www.freedesktop.org/software/systemd/man/latest/systemd.kill.html)
- Use `Restart=on-failure`, `RestartSec=10s`, `StartLimitIntervalSec=300`, and
  `StartLimitBurst=5`. On handled startup errors, stop containers started by the
  failed invocation. Preserve the existing orphan policy for crashes and
  interruptions. The CLI refuses to start when it detects a managed vault
  container orphan in the same installation. Document rerunning the configured
  startup command as the service account with `--gc-containers` once, with the
  service stopped and the same app home, config path and working directory.
  Quit the manual process cleanly before restarting the service, and use
  `systemctl reset-failed brain3` if the service hit its start limit.
  [systemd restart behavior](https://www.freedesktop.org/software/systemd/man/latest/systemd.service.html#Restart=)

## 3. Validate before migrating Arty

- Copy the exact README example, substitute the test account and paths, and
  validate the unit with `systemd-analyze verify` in the existing Ubuntu CI job.
  Run
  `cargo test -p brain3 --no-run`, `cargo test`, and the existing Linux Docker
  MCP smoke workflow. Exercise the unit on a Linux VM with systemd as PID 1 and
  a disposable vault.
- Verify that start and restart complete an authenticated MCP initialize,
  stopping Brain3 releases its listeners and containers, and the tunnel
  service remains active during a Brain3 restart. Add a port-conflict case to
  the existing Linux OAuth smoke test and verify the next start succeeds
  without manual cleanup. Exercise orphan recovery after an interrupted start.
  Verify OAuth metadata and an
  authenticated remote MCP handshake on the public hostname.
- Enable both units, reboot the test host, and confirm remote MCP is ready
  before any interactive login. Use current-boot journal timestamps and
  the public MCP check as evidence.

## 4. Migrate the existing Arty installation

- Follow the same README procedure. Reuse the existing Linux account,
  Brain3 installation, vault, and locally managed named tunnel. Docker is
  already enabled. Install the system package for `cloudflared` and use it
  for the Cloudflare service installer. Stage the existing tunnel
  configuration and credentials for the new unit, preserving its hostname,
  loopback OAuth gateway URL, and DNS route. Preserve the other tunnel
  services on the host.
- Prepare and verify the Brain3 unit and save a private backup of the
  current Brain3 config. Send Ctrl-C to the Brain3 process in tmux and wait
  for it to release its listeners and stop its tunnel. Switch Brain3 to the
  external public hostname configuration, start the Brain3 unit, and
  install and start the Cloudflare unit using the existing tunnel. Verify
  local and public MCP, then confirm both units are enabled.
- If the cutover fails, stop and disable the units introduced for Brain3,
  confirm Brain3 released the runtime, restore the prior Brain3 config, and
  start `brain3 --cli` in tmux with the same account and app home. Verify
  the previous public endpoint works again.
