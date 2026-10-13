# Third-party notices

The controller's original Rust source is MIT licensed. It depends on separately distributed crates whose versions and integrity are recorded in Cargo.lock. Principal dependencies include Axum, Tokio, rusqlite, Serde, reqwest, Clap, Ratatui, Crossterm, Chrono, UUID, SHA-2 and Schemars. Most use MIT and/or Apache-2.0; rusqlite's bundled SQLite is public domain and crypto dependencies have their own notices. Consult distributed dependency licenses when redistributing the binary.

`contracts/plugin-0.1/` contains generated JSON Schemas from the MIT-licensed OpenCode Factory Plugin, version 0.1.1, source commit 7cfea868c961b90bfeca5399b1246f157e70ee97, at https://github.com/lhemerly/opencode-factory-plugin. Copyright (c) 2026 OpenCode Factory Plugin contributors. Its MIT notice is preserved in `contracts/plugin-0.1/LICENSE`.

The controller uses the public OpenCode HTTP interface; it does not copy OpenCode implementation source. OpenCode and its plugin SDK use MIT. Test scripts require separately installed OpenCode/companion plugin packages and do not bundle them into the Rust binary.
