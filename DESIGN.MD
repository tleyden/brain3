
## PRD

See PRD.MD

## Design Sketch

See `Architecture.excalidraw`.

## Design Decisions

### DD: Swift vs Tauri2?

#### Option 1: Swift with SwiftUI 

##### Strengths 

- Simpler
- Can build slick animations and take full advantage of apis

##### Risks

- Doesn't meet requirement to run on headless linux server
- Windows Port

#### Option 2: Swift with React UI in Webview

##### Strengths 

##### Risks

#### Option 3: Tauri2  (rec'd)

##### Strengths 

##### Risks

- A lot of language interop annoyance with Rust -> Swift

### DD: Run in a container?  Which container technology?

#### Option 1: Yes with macos native container, but allow opt out.  (rec'd)

### DD: Is container mandatory or optional?

#### Option 1: Mandatory (mvp)

##### Strenghts

* Simplicity
##### Risks

* How could someone run https://github.com/CursorTouch/MacOS-MCP in a linux container?

#### Option 2: Everything uses container by default, opt out (rec'd)

The container is the zoo grounds!  If you let the agents out of the zoo, they are basically like animals that can run around your whole house.

### DD: How to schedule MCP -> Container 

#### Option 1: One container for all MCP

##### Risks

* Need another isolation mechanism, like nix.  This is Matryoshka doll level complexity, and for what?  Feels like premature optimization for resource sharing.

#### Option 2: Each MCP gets own container (rec'd)

##### Strengths 

* So much simpler
* Normalizes mcps packaged as oci containers with npm/pypi mcp servers

#### Option 3: Monolith container by default, can isolate on case-by-case basis 

##### Risks

* Same as option 1

#### Option 4: Isolated container by default, can opt into a monolith container (shared cage per zoo animals) 

##### Risks

* Same as option 1


### DD: How do we expose MCP servers as separate entities with own tools?

In other words, lets say we're running five separate mcp servers in the zoo, and we want to expose 3 of them?  And we only want to configure one in Claude and all three in ChatGPT.

There is some prior art here with [CF MCP Portals](https://developers.cloudflare.com/cloudflare-one/access-controls/ai-controls/mcp-portals), where they have a single MCP that can aggregate multiple MCP servers.

#### Option 1: One CF Tunnel Gateway that exposes all as a single MCP Server

##### Strengths 

* The user can manage tools in LLM and turn off the ones they don't want
* On the Brain3 side, only MCPs explicitly marked as "remote tunnel" would be exposed.  This is the public entrance to Brain3: reachable from the outside only when access is intentionally allowed.

##### Risks

* Forces us (I think) to move OAuth handling up one layer in the stack.  Needs investigation.
* Context pollution

#### Option 2: CF Tunnel Gateway per MCP Server 

##### Strengths

* Might be simplest in the sense that we let MCP servers handle their own OAuth, and we just setup CF tunnels


##### Risks

* Tunnel cost
* Tunnel management complexity

#### Option 3: Allow multiple CF Tunnel Gateway and arbitrary scheduling to MCP Server

##### Risks

* Option 2 risks + additional comlexity to maintain routing layer

### DD: Which tunneling service?  CF Tunnels or ngrok

#### Option 1: CF Tunnels

##### Strengths

* Generous free tier
* Many related projects recommend it:
  * Open WebUI
  * Flowise
  * Langflow
  * n8n
  * Home Assistant

##### Risks

* Do they require a domain?

#### Option 2: ngrok

##### Strengths

* Definitely don't require a domain

#### Option 3: both

Support both, leave it up to user to decide.

### DD: How to provide TLS Termination?

#### Option 1: CF Proxy Domain 

Similar to jimprosser/obsidian-web-mcp, which solves TLS termination by utilizing a Cloudflare Tunnel (which operates via a Cloudflare Proxy Domain)

##### Risks

* Definitely requires a domain

#### Option 2: ngrok

### DD: How do MCP servers get installed into the monolith container?

#### Option 1: Discover tooling, then run standard install commands

There are some interesting security implications here, especially if users can install outside of the container.  For security and predictability reasons, it's probably better to avoid building an agent into the app to do this.

Instead: create a prompt with instructions that they can copy into their locally running agent.  Give recommendations based on detected agents.  Or even call the agent via CLI to save them a copy and paste, but give warnings first.

### DD: What about MCP servers packaged as docker containers?

#### Option 1: Force these to run in separate container

### DD: Which base container OS to use?

#### Option 1: Alpine Linux

#### Option 2: NixOS

#### Option 3: Debian/Ubuntu


### DD: Where does OAuth happen?

#### Option 1: In a gateway reverse proxy (rec'd)

#### Option 2: In each container (straw)


### DD: How to build OAuth gateway reverse proxy?

#### Option 1: Build from scratch

#### Option 2: oauth2-proxy

##### Risks

* No mcp support https://github.com/oauth2-proxy/oauth2-proxy/issues/3167



### DD: How to build MCP gateway reverse proxy?

#### Option 1: Build from scratch

#### Option 2: Re-use existing MCP gateway



## POC Prototyping

### Derisk: How to handle OAuth2 gateway stuff?

* Can a separate gateway handle OAuth2, then proxy to underlying MCP servers without any auth? 

#### MCP Servers to validate

##### 1. hello-mcp - http, no auth 

##### 2. obsidian-web-mcp - http oauth client id/secret

* jimprosser/obsidian-web-mcp
  * Requires auth by default
  * Could potentially set up a CF tunnel directly in the container that connects to the port, and it might "just work".  No middleman.

##### 3. Flasher - http oauth with supbase login

##### 4. Codex wrapped up as MCP

* https://github.com/teabranch/agentic-developer-mcp

##### Rest+  Maybe look at these

* https://github.com/iFurySt/open-codex-computer-use
* https://github.com/iFurySt/open-browser-use
* wonderwhy-er/DesktopCommanderMCP
  * Stdio by default
  * Maybe we would have to add our own OAuth2 layer and FastMCP wrapper
* microsoft/playwright-mcp
  * Transport: `stdio` by default, HTTP via `--port`, SSE is supported programmatically, HTTPS appears to require external TLS termination or a reverse proxy.
  * Auth: no built-in OAuth2 support for the MCP server itself.
  * Uses a custom token flow for the browser extension connection flow.
  * For web session auth, it relies on Playwright `storageState`; `secrets` helps hide credentials from the LLM but is not an auth layer.
* screenpipe/screenpipe
  * Transport: `stdio` by default and HTTP via `screenpipe-mcp-http`; HTTP endpoint is `/mcp` and also exposes `/health`.
  * HTTP mode is currently partial: `stdio` has the full tool set, while HTTP currently exposes `search_content` only.
  * Auth: no OAuth2 support for MCP auth; repo OAuth code is for other integrations, not the MCP server.
  * HTTP auth uses bearer tokens for non-loopback/LAN access via `--api-key`; loopback access is allowed without auth.


### Derisk: Can we install and manage containers with virtualization.kit?

Probably pretty low risk
