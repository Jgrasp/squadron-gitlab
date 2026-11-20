# GitLab

A [Squadron](https://github.com/Jgrasp/squadron) extension that brings the merge requests and pipelines of your GitLab projects into Squadron, on gitlab.com or on your own GitLab server.

## Use

In Settings › Extensions › GitLab, add an account with the address of your GitLab, such as `gitlab.com` or `gitlab.acme.io`, and a [personal access token](https://gitlab.com/-/user_settings/personal_access_tokens?name=Squadron&scopes=api) named Squadron with the `api` scope, created on that GitLab. **Test the connection** checks the token before you save it.

Add one account per GitLab server. Several accounts on the same server work too: for each group, the extension remembers the account that reached it. The server must answer over HTTPS on the default port, at the root of its address.

Every project whose `origin` is on `gitlab.com` or on the server of one of your accounts then gets, in Squadron:

- the merge request of each agent's branch, opened from Squadron once the branch is pushed, as a draft if you like, and chips for the merge request links in conversations;
- the merge requests to review, yours and the others, with their pipeline and reviewers;
- reviews with line comments, a summary and an approval, replies to remarks, resolved threads, merges with the squash and source branch deletion the project allows, and title and description changes, from you or from an agent;
- the pipelines, with a notification when one ends.

GitLab has no "request changes" verdict for Squadron: a review either approves or comments.

## What it asks Squadron for

- HTTPS requests to `gitlab.com`, and to the server you typed in each account, for GitLab's API.
- GitLab accounts: each token stays in the system keyring. Squadron adds it to the requests to that account's server only, and the extension never reads it.
  - `api` reads the merge requests, their discussions, approvals and pipelines, tells which ones you review or wrote, shows your name and avatar, and opens, comments, approves, merges and edits them.

## Build

```sh
rustup target add wasm32-wasip2
cargo build --release --target wasm32-wasip2
cargo test
```

CI checks the formatting of every push and pull request with `cargo fmt --check`.
