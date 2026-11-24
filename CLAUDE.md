# squadron-gitlab

The `gitlab` extension of Squadron: GitLab merge requests and pipelines, on gitlab.com and self-hosted servers, moved out of Squadron's core. Read `docs/PLAN.md` before anything. `../squadron-bitbucket` is the extension to copy, `../squadron-github` the first one.

This session works on this repo only. The plan's steps in `../squadron` (removing GitLab from the core) and in `squadron-extensions` (the `registry.json` entry, any API change) go to an agent on that project through `start_agent`: never change, run or publish them from here. Same for any problem found in the app or the registry.

```bash
cargo build --release --target wasm32-wasip2
cargo test
```
