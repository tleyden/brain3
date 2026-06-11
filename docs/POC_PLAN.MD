Validate the full stack:

1. Take the obsidian-web-mcp tools and strip out auth, install them in a container and run the container.  Mount a host dir with junk data.
2. Setup networking so that the host process can reverse proxy into the container (no auth yet)
3. Test local mcp via http with no auth, into the container to call tools
4. Extract all the oauth code from obsidian-web-mcp into a separate .py, run it via cli
5. Test oauth2.1 handshake with mcp inspector
6. Add CF tunnel
7. Connect Claude mobile and desktop

Once that's all working, we should have all we need to wrap up an mvp.