

A simple, focused desktop app for discovering and running MCP servers in secure containers, and exposing them to the internet.  Similar to https://www.josh.ing/mymcp, but with:

1. Robust security options, such as native MacOS containers
2. Built-in tunneling + OAuth2 support, similar to what is offered by [Desktop Commander Remote MCP](https://mcp.desktopcommander.app/)

The type of MCP servers that would be potentially interesting:

1. Remotely access local data or desktop apps.  For example, your Obsidian vault or your locally stored claude session history, from Claude/ChatGPT mobile.
2. Access private data that people want to primarily save locally, for example, their local filesystems.
3. Non-hosted MCP servers.  For example, Linear, Notion and Logfire all have their own MCP servers, so there would be no need to run locally.   

### Out of scope

- No model or LLM running - this only does mcp server hosting.  Bring your own LLM with ChatGPT/Claude/LMStudio.

### What kind of MCP servers should be supported?

1. Accesss local obsidian vault or other local files via any ChatGPT/Claude app, including the web/mobile apps.  There is already a fastmcp server that does this.
2. Access local services, like screenpipe.
3. Remote computer control.  For example there is https://github.com/CursorTouch/MacOS-MCP - this would be a perfect MCP to run, 

### Specific MCP servers that are interesting

- Screenpipe
- Desktop Commander (though they have their own Remote MCP: https://mcp.desktopcommander.app/, which is interesting) 

