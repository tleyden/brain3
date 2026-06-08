# POC Tunnel Setup

If the goal is to give a local MCP server a public HTTPS URL quickly, `ngrok` is the easiest option.

For the `oauth2-host-gw` POC, the default local port is `8421`. Replace it below only if you changed `OAUTH2_GATEWAY_PORT`.

## Nix

If you want to install tunnel clients on Linux with Nix, install Nix first:

```bash
curl --proto '=https' --tlsv1.2 -sSf -L https://install.determinate.systems/nix | sh -s -- install
```

Then either start a new shell or run:

```bash
source ~/.bashrc
```

Verify:

```bash
nix --version
```

With Nix installed, you can install either tunnel client without adding extra apt repositories:

```bash
nix profile install nixpkgs#cloudflared
nix profile install nixpkgs#ngrok
```

## ngrok

Install:

### macOS

```bash
brew install ngrok
```

### Linux

```bash
nix profile install nixpkgs#ngrok
```

Verify:

```bash
ngrok version
```

If you do not want to install it permanently:

```bash
nix run nixpkgs#ngrok
```

Or:

```bash
nix shell nixpkgs#ngrok
```

Log in once:

1. Sign up or log in: `https://dashboard.ngrok.com/signup`
2. Open your authtoken page: `https://dashboard.ngrok.com/get-started/your-authtoken`
3. Copy the token
4. Run:

```bash
ngrok config add-authtoken YOUR_TOKEN
```

Run:

```bash
ngrok http 8421
```

You will get a public HTTPS URL like:

```text
https://abc123.ngrok-free.app
```

This gives you:

- HTTPS
- A valid certificate
- Public internet access
- A stable tunnel while the process is running

No domain is required.

For Brain3, installing both clients side-by-side is quick:

```bash
nix profile install nixpkgs#cloudflared
nix profile install nixpkgs#ngrok
```

## Cloudflare Tunnel via trycloudflare (easy, no domain or account required)

Install:

### macOS

```bash
brew install cloudflared
```

### Linux

```bash
nix profile install nixpkgs#cloudflared
```

Run:

```bash
cloudflared tunnel --url http://localhost:8421
```

This gives you:

- A temporary public HTTPS `*.trycloudflare.com` URL
- No token required
- No account required
- No domain required

This is temporary: it lasts only while the `cloudflared` process is running, and it is best used for testing or quick demos.

Unlike `ngrok`, Cloudflare Quick Tunnels do not require login for this basic flow. `ngrok` still requires `ngrok config add-authtoken YOUR_TOKEN` for normal authenticated use.

## Cloudflare Tunnel with Custom Domain

Note: domain required.

For a permanent hostname such as `https://mcp.yourdomain.com`, Cloudflare Tunnel usually requires:

- A Cloudflare account
- A domain managed in Cloudflare DNS

Then you create a named tunnel and route DNS to it.
