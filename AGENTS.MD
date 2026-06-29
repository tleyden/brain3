# General 

- The point of the app: Brain3 is a local-first, markdown-based knowledge management system that stores information in an Obsidian-compatible repository and exposes it to AI clients such as ChatGPT and Claude through an MCP interface.
- Also the other point of the app is to open it up to remote access since that what Claude and ChatGPT demand for access from mobile and web.
- This works on both linux and macos.  On macos, we can use macos native containers or docker.  On linux, docker only.
- Don't suggest subagents.  We will always do this in smaller steps in serial to manage context window rather than spawning subagents.
- Don't commit to git, I will commit.
- Security rule: this gateway is intentionally closed to preregistered clients only. Do not enable CIMD, DCR, public-client OAuth (`token_endpoint_auth_method=none`), or any other flow that allows access without the hardcoded client ID and client secret, unless explicitly requested by the user. Do not advertise OAuth capabilities the server is not supposed to allow, and do not expose `/oauth/register` in a way that hands out usable credentials or broadens access. Any auth change must preserve the policy that only explicitly preregistered clients can obtain tokens and reach protected MCP data.
- Keep apps/gateway/src/main.rs as lean as possible.  Only add code that would otherwise be awkward in platform or core crates. 
- To support  remote mcp with https and oauth2.1, as many AI apps require, the gateway must have a public ingress via a cloudflare tunnel or otherwise.
- We need fairly verbose logging and tracing to diagnose errors.  Just use the appropriate log level so we can control the noise.  Without this, its a black box.
- Everything in the poc dir is dead legacy.  Ignore it unless specifically asked.  The top level rust project and brain3-mcp-vault-tools are the only active codebases.
- Run cargo test, which should pass, before considering a change as finished.
- For Python tests in `brain3-mcp-vault-tools`, run: `cd brain3-mcp-vault-tools && uv run --with mcp python -m unittest discover -s tests -v`. This discovers and runs all tests in the `tests/` directory. (`pytest` is not installed; `uv run --with mcp python -m unittest` is the correct runner.)
- This project is new, so default to verbose logging.  Of course, never log secrets or sensitive data.
- This project is highly sensitive from a security point of view, don't introduce new points of ingress without first updating the threat model in SECURITY_AUDIT.MD under the Threat Model section.

# Rust Development & Hexagonal Architecture: Best Practices

Based on the provided sources, here is a summary of best practices for Rust development and the implementation of hexagonal architecture.

## Rust Best Practices

- **Prioritize Type Safety and Expressiveness:** Leverage Rust's extensive type system, including sum types (enums) and the ownership model, to model domain logic nuance and ensure memory safety.
- **Idiomatic Error Handling:** Use `Result` for recoverable errors and reserve `panic!` for unrecoverable programmer errors or critical system failures.
- **Effective API Design:** Follow established guidelines for naming, documentation, and predictability to ensure crates are interoperable and intuitive for the ecosystem.
- **Asynchronous Efficiency:** Utilize mature runtimes like Tokio for high-performance concurrent tasks. Avoid performing blocking I/O inside asynchronous functions to prevent stalling the executor.
- **Contain External Dependencies:** Wrap third-party crates (like database clients or web frameworks) in internal types to prevent implementation details from leaking into the core application logic.
- **Database Management:** Use connection pooling (e.g., `sqlx::Pool`, `r2d2`) for efficient access and manage state changes within transactions to maintain data integrity.
- **Robust Testing, focused on public or internal API boundaries:** Implement a mix of unit tests for business logic and integration tests for external boundaries. Use mocking libraries like `mockall` to simulate dependency behavior during testing.  Be very judidicous about writing unit tests, they create a lot of tech debt.  Never write tests for log output for example.  It must just be core functionality on public apis.  Never test private apis, because they constantly break.
- **Leverage Design Patterns:** Use the Newtype pattern to encapsulate data and enforce domain-specific validation at the type level.

## Hexagonal Architecture Best Practices

- **Maintain Core Independence:** The domain (core) should contain all business rules and entities while remaining blissfully ignorant of external details like databases, UIs, or frameworks.
- **Define Ports as Traits:** Use Rust traits to define Ports (interfaces). These act as stable contracts that dictate how the domain interacts with the outside world.
- **Implement Adapters for Details:** Adapters are concrete implementations of ports (e.g., a Postgres adapter for a repository port). They handle technical specifics without affecting the domain.
- **Enforce Inward Dependency Flow:** Ensure all source code dependencies point toward the inner domain; the core must never know anything about the outer layers.
- **Decouple Data Models:** Create unique domain models for business logic. These should be distinct from transport-layer models (like JSON request/response structs) to allow both to evolve independently.
- **Orchestrate via Services:** Use a Service layer to coordinate use cases. Services call multiple ports (e.g., saving to a repository and then sending a notification) without knowing how those actions are implemented.
- **Optimize for Testability:** Because the domain relies on traits (ports), you can easily plug in mock adapters to test business logic in complete isolation from the real world.
- **Start with Large Domain Boundaries:** When starting, prefer a single, large domain rather than many small ones. This simplifies atomicity and allows domain boundaries to be refined organically based on actual usage patterns.
- **Expose a Clear Public API:** Inbound adapters (like an HTTP router) should only interact with the Service API, which acts as the primary point of entry for the application's business logic.

## Updating new release

- Do not update anything in docs folder.  Just update these files: Cargo.toml, main.rs, release.rs, screens.rs, pyproject.toml, test_server_startup.py, uv.lock, first_run_setup.rs, and importantly README.md

## Planning

- When creating plans, write the plan into docs/superpowers/plans as markdown files.
