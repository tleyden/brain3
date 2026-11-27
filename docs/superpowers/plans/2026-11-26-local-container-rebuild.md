# Rebuild the Brain3 vault-tools container locally

## Goal

Give developers one Brain3 command to build the vault-tools image from the current checkout and start the configured gateway with it. The command should work for staging in one shell using the existing tunnel and vault configuration. The image stays in the configured local container runtime. A developer can run the command without a registry push or GitHub Actions.

Today, the setup wizard configures the runtime and image. `--container-tag` selects an existing tag. The vault-tools developer README documents `scripts/build-container.sh` for building an image and a second script for running it separately. The normal gateway startup uses a local image when it finds the configured tag and pulls the image when the tag is missing. It never checks the checkout for changes.

## 1. Add a foreground rebuild option

- Add `--rebuild-container` to the Brain3 CLI. From the checkout root, run `brain3 --rebuild-container --brain3-home "$HOME/.brain3_staging"` with a Brain3 binary built from the implementation branch. The flag selects foreground CLI startup, and `--cli` is also accepted. Keep the setup wizard for choosing the runtime and configuring Brain3 on first use.
- Require an existing Brain3 configuration with a managed vault-tools runtime and a checkout containing `brain3-mcp-vault-tools/Containerfile`. Validate these before starting the builder. Give actionable errors for a missing config, missing source, unavailable runtime, or a configuration that would use bind-mounted source or a different upstream. Reject combinations with `--setup`, `--cloudflare-setup`, `--tui`, and `--container-tag`.
- Build on each explicit request and use the builder’s layer cache. The normal start command continues to use the configured image. Leave automatic freshness detection and a wizard action for a later decision after the command has been used in staging.

## 2. Build before starting the gateway

- Keep argument parsing and launch dispatch in `apps/gateway/src/main.rs`. Put the build logic in the platform container module. Follow the existing build script’s runtime mappings. Use Apple `container build` with `macos-container` and `docker build` with `docker`. Give the process the vault-tools directory as its build context and pass the `Containerfile` and tag as separate arguments.
- Use a generated local image reference for each build so concurrent builds cannot replace the image another launch is about to use. Verify the image exists in the selected runtime before continuing. Apply the reference to the primary vault-tools container in memory for that launch. Treat a missing build output as a startup error. Keep the selected vault, credentials, and Cloudflare configuration from the existing installation.
- Show build progress in the foreground. Return the builder failure before starting either the gateway or the tunnel. Feed a successful build through the existing container startup, health probe, and shutdown paths so Brain3 owns the container lifecycle.

## 3. Document and verify the command

- Put the one-command staging example under **Start Brain3** in the top-level README. Link to it from **Container Build** in the vault-tools README. Keep the existing build-only script documented for developers who need an image without launching Brain3.
- Test the public CLI with a stand-in builder that records its arguments. Verify the selected runtime and build context, early errors, and that a failed build stops startup. Run `cargo test -p brain3 --no-run` and `cargo test` on macOS.
- Extend the existing Linux Docker smoke coverage so the rebuild invocation produces its own image and serves a vault read through the normal gateway startup. Run the container E2E test on GitHub Actions per the repository’s Linux-container testing rule. Day-to-day staging rebuilds use the local builder.
- Smoke-test staging with a disposable vault. Confirm one foreground command starts the managed container and existing staging tunnel. Connect an OAuth client, read a fixture note, then stop Brain3 and verify it stops the container it launched.

## Delivery order

Keep this plan on `deps-updates` in PR #180. For implementation, fast-forward `main`, create a fresh branch from `main`, and open a standalone PR with the rebuild command, docs, and tests. Merge that PR first, then update `deps-updates` from `main` and smoke-test the dependency changes in staging with the new command.
