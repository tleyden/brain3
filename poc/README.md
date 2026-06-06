# POC Tunnel Setup

If the goal is to give a local MCP server a public HTTPS URL quickly, `ngrok` is the easiest option.

For the `oauth2-host-gw` POC, the default local port is `8421`. Replace it below only if you changed `OAUTH2_GATEWAY_PORT`.

## ngrok

Install:

```bash
brew install ngrok
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

## TryCloudflare

Install:

```bash
brew install cloudflared
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

## Cloudflare Tunnel

Note: domain required.

For a permanent hostname such as `https://mcp.yourdomain.com`, Cloudflare Tunnel usually requires:

- A Cloudflare account
- A domain managed in Cloudflare DNS

Then you create a named tunnel and route DNS to it.
