use crate::api;
use crate::reads::{self, Repo};
use serde_json::{Map, Value, json};
use squadron_extension_api::forge::{
    Draft, MergeArgs, PostArgs, Posted, ReplyArgs, ResolveArgs, UpdateArgs,
};

pub const NO_REQUEST_CHANGES: &str =
    "The comments are on GitLab, but Squadron cannot request changes there: do it on GitLab.";

fn send(repo: &Repo, method: &str, path: &str, body: Option<Value>) -> Result<Value, String> {
    api::send(&repo.account, &repo.site, method, path, body)
}

pub fn reply(repo: &Repo, args: &ReplyArgs) -> Result<(), String> {
    let path = repo.request(args.number);
    let url = match reads::thread(repo, args.number, &args.to)? {
        Some(id) => format!("{path}/discussions/{id}/notes"),
        None => format!("{path}/notes"),
    };
    send(repo, "POST", &url, Some(json!({ "body": args.body }))).map(|_| ())
}

pub fn resolve(repo: &Repo, args: &ResolveArgs) -> Result<(), String> {
    let Some(id) = reads::thread(repo, args.number, &args.to)? else {
        return Err(format!("GitLab has no thread with the note {}.", args.to));
    };
    let resolved = if args.resolved { "true" } else { "false" };
    let path = api::query(
        &format!("{}/discussions/{id}", repo.request(args.number)),
        &[("resolved", resolved)],
    );
    send(repo, "PUT", &path, None).map(|_| ())
}

pub fn draft_of(draft: &Draft, refs: &Value) -> (&'static str, Value) {
    if draft.path.is_empty() {
        return ("notes", json!({ "body": draft.body }));
    }
    let mut position = json!({
        "position_type": "text",
        "base_sha": refs["base_sha"],
        "start_sha": refs["start_sha"],
        "head_sha": refs["head_sha"],
        "new_path": draft.path,
        "old_path": draft.path,
    });
    position[if draft.removed {
        "old_line"
    } else {
        "new_line"
    }] = json!(draft.line);
    (
        "discussions",
        json!({ "body": draft.body, "position": position }),
    )
}

pub fn post(repo: &Repo, args: &PostArgs) -> Result<Posted, String> {
    let path = repo.request(args.number);
    let refs = repo.get(&path)?["diff_refs"].take();
    for (done, draft) in args.drafts.iter().enumerate() {
        let (kind, body) = draft_of(draft, &refs);
        if let Err(error) = send(repo, "POST", &format!("{path}/{kind}"), Some(body)) {
            return Ok(Posted {
                done,
                said: false,
                error: Some(error),
            });
        }
    }
    let summary = args.summary.trim();
    let mut sent = if summary.is_empty() {
        Ok(Value::Null)
    } else {
        send(
            repo,
            "POST",
            &format!("{path}/notes"),
            Some(json!({ "body": summary })),
        )
    };
    let said = sent.is_ok();
    match args.verdict.as_str() {
        "approve" if said => sent = send(repo, "POST", &format!("{path}/approve"), None),
        "request_changes" if said => sent = Err(NO_REQUEST_CHANGES.into()),
        _ => {}
    }
    Ok(Posted {
        done: args.drafts.len(),
        said,
        error: sent.err(),
    })
}

pub fn merge_options(args: &MergeArgs) -> Value {
    let mut options = Map::new();
    if let Some(method) = &args.method {
        options.insert("squash".into(), json!(method == "squash"));
    }
    if let Some(delete) = args.delete_branch {
        options.insert("should_remove_source_branch".into(), json!(delete));
    }
    Value::Object(options)
}

pub fn merge_steps(path: &str, args: &MergeArgs) -> Vec<(String, Value)> {
    let mut steps = Vec::new();
    if args.method.as_deref().is_some_and(|m| m != "squash") {
        steps.push((path.to_string(), json!({ "squash": false })));
    }
    steps.push((format!("{path}/merge"), merge_options(args)));
    steps
}

pub fn merge(repo: &Repo, args: &MergeArgs) -> Result<(), String> {
    for (path, body) in merge_steps(&repo.request(args.number), args) {
        send(repo, "PUT", &path, Some(body))?;
    }
    Ok(())
}

pub fn changes(args: &UpdateArgs) -> Value {
    let mut changes = Map::new();
    if let Some(title) = &args.title {
        changes.insert("title".into(), json!(title));
    }
    if let Some(description) = &args.description {
        changes.insert("description".into(), json!(description));
    }
    Value::Object(changes)
}

pub fn update(repo: &Repo, args: &UpdateArgs) -> Result<(), String> {
    send(repo, "PUT", &repo.request(args.number), Some(changes(args))).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draft(path: &str, line: u32, removed: bool, body: &str) -> Draft {
        Draft {
            path: path.into(),
            line,
            removed,
            body: body.into(),
        }
    }

    #[test]
    fn writes_line_comments_on_the_new_or_old_side() {
        let refs = json!({ "base_sha": "b", "start_sha": "s", "head_sha": "h" });
        assert_eq!(
            draft_of(&draft("src/a.go", 4, false, "Rename"), &refs),
            (
                "discussions",
                json!({ "body": "Rename", "position": { "position_type": "text", "base_sha": "b", "start_sha": "s", "head_sha": "h", "new_path": "src/a.go", "old_path": "src/a.go", "new_line": 4 } })
            )
        );
        let (_, old) = draft_of(&draft("src/a.go", 9, true, "Why?"), &refs);
        assert_eq!(
            (
                &old["position"]["old_line"],
                old["position"].get("new_line")
            ),
            (&json!(9), None)
        );
        assert_eq!(
            draft_of(&draft("", 0, false, "Add a test."), &refs),
            ("notes", json!({ "body": "Add a test." }))
        );
    }

    #[test]
    fn merges_with_the_chosen_squash_and_branch_deletion() {
        let args = |method: Option<&str>, delete_branch| MergeArgs {
            number: 3,
            method: method.map(str::to_string),
            delete_branch,
        };
        assert_eq!(merge_options(&args(None, None)), json!({}));
        assert_eq!(
            merge_options(&args(Some("squash"), Some(true))),
            json!({ "squash": true, "should_remove_source_branch": true })
        );
        assert_eq!(
            merge_options(&args(Some("rebase"), Some(false))),
            json!({ "squash": false, "should_remove_source_branch": false })
        );
        let paths = |method| {
            merge_steps("/mr/3", &args(method, None))
                .into_iter()
                .map(|(path, _)| path)
                .collect::<Vec<_>>()
        };
        assert_eq!(paths(Some("merge")), ["/mr/3", "/mr/3/merge"]);
        assert_eq!(paths(Some("squash")), ["/mr/3/merge"]);
        assert_eq!(paths(None), ["/mr/3/merge"]);
    }

    #[test]
    fn sends_only_what_changes() {
        let title = UpdateArgs {
            number: 3,
            title: Some("Export".into()),
            description: None,
        };
        assert_eq!(changes(&title), json!({ "title": "Export" }));
        let both = UpdateArgs {
            number: 3,
            title: Some("Export".into()),
            description: Some("Adds export".into()),
        };
        assert_eq!(
            changes(&both),
            json!({ "title": "Export", "description": "Adds export" })
        );
    }
}
