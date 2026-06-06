# POC Tunnel Setup

If the goal is to give a local MCP server a public HTTPS URL quickly, `ngrok` is the easiest option.

Replace `8080` below with the port your local server is actually using.

## ngrok

Install:

```bash
brew install ngrok
```

Log in once:

```bash
ngrok config add-authtoken YOUR_TOKEN
```

Run:

```bash
ngrok http 8080
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

## Cloudflare Tunnel

Note: domain required.

For a permanent hostname such as `https://mcp.yourdomain.com`, Cloudflare Tunnel usually requires:

- A Cloudflare account
- A domain managed in Cloudflare DNS

Then you create a named tunnel and route DNS to it.
