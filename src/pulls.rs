use serde_json::Value;
use squadron_extension_api::forge::{Pull, Reviewer};

pub fn text(value: &Value) -> String {
    value.as_str().unwrap_or_default().to_string()
}

fn verdict(state: &str) -> &'static str {
    match state {
        "approved" => "approved",
        "requested_changes" => "changes_requested",
        "reviewed" => "commented",
        _ => "waiting",
    }
}

fn checks(pipeline: &Value) -> Option<String> {
    let state = match pipeline["status"].as_str()? {
        "success" | "skipped" | "manual" => "passing",
        "failed" | "canceled" => "failing",
        _ => "pending",
    };
    Some(state.into())
}

pub fn pull_of(found: &Value, reviewers: &Value, approvals: &Value, me: &str) -> Pull {
    let approved: Vec<&Value> = approvals["approved_by"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|a| &a["user"])
        .collect();
    let mut people: Vec<(&Value, &str)> = match reviewers.as_array() {
        Some(list) if !list.is_empty() => list
            .iter()
            .map(|r| (&r["user"], verdict(r["state"].as_str().unwrap_or_default())))
            .collect(),
        _ => found["reviewers"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|user| (user, "waiting"))
            .collect(),
    };
    for user in &approved {
        match people.iter_mut().find(|(u, _)| u["id"] == user["id"]) {
            Some(person) => person.1 = "approved",
            None => people.push((user, "approved")),
        }
    }
    let state = match found["state"].as_str().unwrap_or_default() {
        "opened" => "open",
        "locked" => "closed",
        other => other,
    };
    let draft = found["draft"] == true;
    let mine = |u: &Value| !me.is_empty() && u["username"] == me;
    Pull {
        number: found["iid"].as_u64().unwrap_or_default(),
        title: text(&found["title"]),
        description: text(&found["description"]),
        url: text(&found["web_url"]),
        state: state.into(),
        draft,
        approvals: approved.len() as u32,
        changes_requested: people.iter().any(|(_, v)| *v == "changes_requested"),
        comments: found["user_notes_count"].as_u64().unwrap_or_default() as u32,
        mergeable: state == "open" && !draft && found["detailed_merge_status"] == "mergeable",
        checks: checks(&found["head_pipeline"]),
        branch: text(&found["source_branch"]),
        base: text(&found["target_branch"]),
        head: text(&found["sha"]),
        author: text(&found["author"]["name"]),
        author_avatar: text(&found["author"]["avatar_url"]),
        updated: text(&found["updated_at"]),
        reviewed: people
            .iter()
            .find(|(u, _)| mine(u))
            .map(|(_, v)| *v)
            .filter(|v| *v != "waiting")
            .map(str::to_string),
        reviewed_head: None,
        reviewed_at: approvals["approved_by"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|a| mine(&a["user"]))
            .and_then(|a| a["approved_at"].as_str())
            .map(str::to_string),
        head_at: None,
        reviewers: people
            .iter()
            .map(|(u, v)| Reviewer {
                name: text(&u["name"]),
                avatar: text(&u["avatar_url"]),
                verdict: v.to_string(),
            })
            .collect(),
        conflicts: found["has_conflicts"] == true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sums_up_a_merge_request() {
        let user = |id: u64, username: &str, name: &str| json!({ "id": id, "username": username, "name": name, "avatar_url": format!("https://gitlab.com/{username}.png") });
        let found = json!({
            "iid": 7767, "title": "go: Update module k8s.io/api to v0.37.2", "description": "Bump", "state": "opened", "draft": false,
            "detailed_merge_status": "mergeable", "has_conflicts": false, "user_notes_count": 2,
            "source_branch": "renovate/k8s.io-api-0.x", "target_branch": "main", "sha": "2c7a3330",
            "web_url": "https://gitlab.com/gitlab-org/gitlab-runner/-/merge_requests/7767", "updated_at": "2026-11-04T09:32:42.995Z",
            "head_pipeline": { "status": "running" },
            "reviewers": [user(8111826, "ratchade", "Romuald Atchadé"), user(10764887, "vtak", "Vishal Tak")],
            "author": user(8420142, "gitlab-dependency-update-bot", "GitLab Dependency Bot")
        });
        let approvals = json!({ "approved_by": [{ "user": user(10764887, "vtak", "Vishal Tak"), "approved_at": "2026-11-04T09:32:31.669Z" }] });
        let pull = pull_of(&found, &Value::Null, &approvals, "vtak");
        assert_eq!(
            (
                pull.number,
                pull.state.as_str(),
                pull.approvals,
                pull.comments,
                pull.checks.as_deref()
            ),
            (7767, "open", 1, 2, Some("pending"))
        );
        assert_eq!(
            (
                pull.branch.as_str(),
                pull.base.as_str(),
                pull.author.as_str(),
                pull.mergeable
            ),
            (
                "renovate/k8s.io-api-0.x",
                "main",
                "GitLab Dependency Bot",
                true
            )
        );
        assert_eq!(
            (pull.reviewed.as_deref(), pull.reviewed_at.as_deref()),
            (Some("approved"), Some("2026-11-04T09:32:31.669Z"))
        );
        let seen: Vec<_> = pull
            .reviewers
            .iter()
            .map(|r| (r.name.as_str(), r.verdict.as_str()))
            .collect();
        assert_eq!(
            seen,
            [("Romuald Atchadé", "waiting"), ("Vishal Tak", "approved")]
        );
        let states = json!([{ "user": user(8111826, "ratchade", "Romuald Atchadé"), "state": "requested_changes" }]);
        let asked = pull_of(&found, &states, &Value::Null, "ratchade");
        assert!(asked.changes_requested);
        assert_eq!(asked.reviewed.as_deref(), Some("changes_requested"));
        let mut merged = found.clone();
        merged["state"] = json!("merged");
        assert!(!pull_of(&merged, &Value::Null, &Value::Null, "").mergeable);
    }

    #[test]
    fn sums_up_the_head_pipeline() {
        let pull = |status: Value| {
            pull_of(
                &json!({ "head_pipeline": { "status": status } }),
                &Value::Null,
                &Value::Null,
                "",
            )
            .checks
        };
        assert_eq!(pull(json!("manual")).as_deref(), Some("passing"));
        assert_eq!(pull(json!("canceled")).as_deref(), Some("failing"));
        assert_eq!(pull(Value::Null), None);
        let locked = pull_of(
            &json!({ "state": "locked" }),
            &Value::Null,
            &Value::Null,
            "",
        );
        assert_eq!(locked.state, "closed");
    }
}
