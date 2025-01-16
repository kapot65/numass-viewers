## Prepare project
- install toolchain and deps (see [update-stack](update-stack.sh))

## Debug wasm app

1. Form a vscode workspace with the following settings:
    ```json
    // ... 
    "settings": {
            "rust-analyzer.cargo.target": "wasm32-unknown-unknown",
        }
    ```
    this will make rust-analyzer use wasm as default target
    (to modify both native and wasm targets one must open 2 vscode workspaces including workspace above)

 2. From wasm workspace execute 
    ```shell
    trunk serve
    ``` 
    to start a local server for the app
 3. Start numass-server in another terminal window (pref --release flag)
    ```shell
    cd ../numass-server/
    cargo run --release -- /data-fast/numass-server/
    ```
    current [Trunk.toml](Trunk.toml) is configured to use numass-server default port as api proxy so everything should work.


## Workarouds

### VSCode doesn't see numass-* packages in WASM workspace
- delete `git` folder in `~/.cargo` to force cargo to re-fetch
    > [!NOTE]
    > Возможно сработает просто `cargo update`