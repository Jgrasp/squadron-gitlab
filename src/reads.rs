use crate::api;
use crate::pulls::{self, text};
use serde_json::Value;
use squadron_extension_api::forge::{Lists, Numbered, Person, Pull, Remark, Run};
use squadron_extension_api::host;
use std::collections::BTreeMap;

pub struct Repo {
    pub account: String,
    pub site: String,
    pub path: String,
}

pub fn site_and_path(
    repo: &str,
    groups: &BTreeMap<String, String>,
) -> Result<(String, String), String> {
    match (groups.get("site"), groups.get("path")) {
        (Some(site), Some(path)) => Ok((site.to_lowercase(), path.clone())),
        _ => repo
            .split_once('/')
            .filter(|(s, p)| !s.is_empty() && p.contains('/'))
            .map(|(s, p)| (s.to_lowercase(), p.to_string()))
            .ok_or_else(|| format!("{repo} is not a GitLab project.")),
    }
}

impl Repo {
    pub fn project(&self) -> String {
        format!("/projects/{}", self.path.replace('/', "%2F"))
    }

    pub fn request(&self, iid: u64) -> String {
        format!("{}/merge_requests/{iid}", self.project())
    }

    pub fn get(&self, path: &str) -> Result<Value, String> {
        api::send(&self.account, &self.site, "GET", path, None)
    }
}

pub fn username_key(account: &str) -> String {
    format!("username:{account}")
}

pub fn username(repo: &Repo) -> Result<String, String> {
    let key = username_key(&repo.account);
    if let Some(found) = host::state_get(&key) {
        return Ok(found);
    }
    let user = repo.get("/user")?;
    let name = text(&user["username"]);
    if name.is_empty() {
        return Err(api::refused());
    }
    host::state_set(&key, Some(&name));
    Ok(name)
}

fn full_paths(repo: &Repo, iid: u64) -> [String; 3] {
    let path = repo.request(iid);
    [
        path.clone(),
        format!("{path}/reviewers"),
        format!("{path}/approvals"),
    ]
}

pub fn fulls(repo: &Repo, iids: &[u64], me: &str) -> Result<Vec<(Pull, Value)>, String> {
    let paths: Vec<String> = iids.iter().flat_map(|iid| full_paths(repo, *iid)).collect();
    let mut found = api::each(&repo.account, &repo.site, &paths).into_iter();
    let mut pulls = Vec::new();
    while let (Some(request), Some(reviewers), Some(approvals)) =
        (found.next(), found.next(), found.next())
    {
        let request = request?;
        let pull = pulls::pull_of(
            &request,
            &reviewers.unwrap_or_default(),
            &approvals.unwrap_or_default(),
            me,
        );
        pulls.push((pull, request));
    }
    Ok(pulls)
}

fn iids(list: &Value) -> Vec<u64> {
    list.as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| m["iid"].as_u64())
        .collect()
}

fn list_path(repo: &Repo, query: &[(&str, &str)]) -> String {
    api::query(&format!("{}/merge_requests", repo.project()), query)
}

pub fn pull(repo: &Repo, branch: &str) -> Result<Option<Pull>, String> {
    let list = repo.get(&list_path(
        repo,
        &[
            ("source_branch", branch),
            ("order_by", "updated_at"),
            ("per_page", "1"),
        ],
    ))?;
    let Some(iid) = iids(&list).first().copied() else {
        return Ok(None);
    };
    let me = username(repo).unwrap_or_default();
    Ok(fulls(repo, &[iid], &me)?.pop().map(|(pull, _)| pull))
}

pub fn changed_files(found: &Value) -> Option<u64> {
    text(&found["changes_count"])
        .trim_end_matches('+')
        .parse()
        .ok()
}

pub fn numbered(repo: &Repo, number: u64) -> Result<Numbered, String> {
    let me = username(repo).unwrap_or_default();
    let (pull, found) = fulls(repo, &[number], &me)?
        .pop()
        .ok_or_else(|| format!("GitLab has no merge request {number}."))?;
    Ok(Numbered {
        pull,
        files: changed_files(&found),
        ..Numbered::default()
    })
}

pub fn sorted(found: [Vec<u64>; 3]) -> Vec<u64> {
    let [review, mine, open] = found;
    let mut all = review;
    all.extend(mine);
    for iid in open {
        if !all.contains(&iid) {
            all.push(iid);
        }
    }
    all
}

pub fn lists(repo: &Repo) -> Result<Lists, String> {
    let me = username(repo)?;
    let open = [
        ("state", "opened"),
        ("order_by", "updated_at"),
        ("per_page", "50"),
    ];
    let paths: Vec<String> = ["reviewer_username", "author_username", ""]
        .iter()
        .map(|who| {
            let mut query = open.to_vec();
            if !who.is_empty() {
                query.insert(0, (who, me.as_str()));
            }
            list_path(repo, &query)
        })
        .collect();
    let mut found: [Vec<u64>; 3] = Default::default();
    for (i, list) in api::each(&repo.account, &repo.site, &paths)
        .into_iter()
        .enumerate()
    {
        found[i] = iids(&list?);
    }
    let (review, mine) = (found[0].clone(), found[1].clone());
    let mut pulls = fulls(repo, &sorted(found), &me)?
        .into_iter()
        .map(|(pull, _)| pull);
    let review: Vec<Pull> = pulls.by_ref().take(review.len()).collect();
    let mine: Vec<Pull> = pulls.by_ref().take(mine.len()).collect();
    Ok(Lists {
        review,
        mine,
        others: pulls.collect(),
    })
}

fn discussions(repo: &Repo, iid: u64) -> Result<Vec<Value>, String> {
    let path = format!("{}/discussions", repo.request(iid));
    let mut found = Vec::new();
    for page in 1..=20 {
        let page = page.to_string();
        let list = repo.get(&api::query(&path, &[("per_page", "100"), ("page", &page)]))?;
        let list = list.as_array().cloned().unwrap_or_default();
        let last = list.len() < 100;
        found.extend(list);
        if last {
            break;
        }
    }
    Ok(found)
}

pub fn thread(repo: &Repo, iid: u64, to: &str) -> Result<Option<String>, String> {
    let to: u64 = to
        .parse()
        .map_err(|_| format!("{to} is not a GitLab note."))?;
    Ok(discussions(repo, iid)?
        .into_iter()
        .find(|d| {
            d["notes"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|n| n["id"].as_u64() == Some(to))
        })
        .and_then(|d| d["id"].as_str().map(str::to_string)))
}

pub fn remarks_of(discussion: &Value) -> Vec<Remark> {
    let notes: Vec<&Value> = discussion["notes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|n| n["system"] != true)
        .collect();
    let root = notes.first().and_then(|n| n["id"].as_u64());
    let resolvable = notes.first().is_some_and(|n| n["resolvable"] == true);
    let resolved = notes.first().is_some_and(|n| n["resolved"] == true);
    notes
        .iter()
        .filter_map(|note| {
            let id = note["id"].as_u64()?;
            let position = &note["position"];
            let (new, old) = (position["new_line"].as_u64(), position["old_line"].as_u64());
            let path = position["new_path"]
                .as_str()
                .or(position["old_path"].as_str());
            Some(Remark {
                id: id.to_string(),
                author: text(&note["author"]["name"]),
                avatar: text(&note["author"]["avatar_url"]),
                body: text(&note["body"]),
                path: path.map(str::to_string),
                line: new.or(old).map(|n| n as u32),
                removed: new.is_none() && old.is_some(),
                reply_to: root.filter(|r| *r != id).map(|r| r.to_string()),
                at: text(&note["created_at"]),
                kind: if path.is_some() { "line" } else { "general" }.into(),
                resolved: (resolvable && root == Some(id)).then_some(resolved),
                commit: position["head_sha"].as_str().map(str::to_string),
            })
        })
        .collect()
}

pub fn remarks(repo: &Repo, number: u64) -> Result<Vec<Remark>, String> {
    let mut found: Vec<Remark> = discussions(repo, number)?
        .iter()
        .flat_map(remarks_of)
        .collect();
    found.sort_by(|a, b| a.at.cmp(&b.at));
    Ok(found)
}

pub fn person(user: &Value) -> Person {
    Person {
        name: text(&user["name"]),
        avatar: text(&user["avatar_url"]),
    }
}

pub fn me(repo: &Repo) -> Result<Person, String> {
    repo.get("/user").map(|user| person(&user))
}

fn pull_in(reference: &str) -> Option<u64> {
    reference
        .strip_prefix("refs/merge-requests/")?
        .split('/')
        .next()?
        .parse()
        .ok()
}

pub fn run_of(value: &Value) -> Run {
    let status = match value["status"].as_str().unwrap_or_default() {
        "success" => "success",
        "failed" => "failure",
        "canceled" | "canceling" => "cancelled",
        "skipped" => "skipped",
        "running" => "running",
        _ => "queued",
    };
    let source = text(&value["source"]);
    let branch = text(&value["ref"]);
    Run {
        id: value["id"].as_u64().unwrap_or_default().to_string(),
        number: value["iid"].as_u64().unwrap_or_default(),
        name: value["name"]
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| source.clone()),
        title: branch.clone(),
        event: source,
        pull: pull_in(&branch),
        branch,
        url: text(&value["web_url"]),
        started: text(&value["created_at"]),
        finished: matches!(status, "success" | "failure" | "cancelled" | "skipped")
            .then(|| text(&value["updated_at"])),
        status: status.into(),
        author: String::new(),
        author_avatar: String::new(),
    }
}

pub fn runs(repo: &Repo) -> Result<Vec<Run>, String> {
    let path = api::query(
        &format!("{}/pipelines", repo.project()),
        &[("per_page", "20")],
    );
    Ok(repo
        .get(&path)?
        .as_array()
        .into_iter()
        .flatten()
        .map(run_of)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_remarks_of_a_discussion() {
        let discussion = json!({ "id": "6a9c", "notes": [
            { "id": 1, "system": true, "body": "added 1 commit", "author": { "name": "A" }, "created_at": "t0" },
            { "id": 2, "type": "DiffNote", "body": "Rename", "author": { "name": "Ann", "avatar_url": "https://a/ann.png" }, "created_at": "t1", "resolvable": true, "resolved": true, "position": { "new_path": "src/a.go", "old_path": "src/a.go", "new_line": null, "old_line": 4, "head_sha": "a1b2c3d" } },
            { "id": 3, "type": "DiffNote", "body": "Done", "author": { "name": "Bob" }, "created_at": "t2", "position": { "new_path": "src/a.go", "new_line": 4 } }
        ] });
        let found = remarks_of(&discussion);
        let seen: Vec<_> = found
            .iter()
            .map(|r| {
                (
                    r.id.as_str(),
                    r.line,
                    r.removed,
                    r.reply_to.as_deref(),
                    r.kind.as_str(),
                )
            })
            .collect();
        assert_eq!(
            seen,
            [
                ("2", Some(4), true, None, "line"),
                ("3", Some(4), false, Some("2"), "line")
            ]
        );
        assert_eq!(
            (found[0].author.as_str(), found[0].avatar.as_str()),
            ("Ann", "https://a/ann.png")
        );
        let general = json!({ "notes": [{ "id": 9, "body": "LGTM", "author": { "name": "A" }, "created_at": "t" }] });
        assert_eq!(remarks_of(&general)[0].kind, "general");
        assert_eq!(
            (
                found[0].resolved,
                found[0].commit.as_deref(),
                found[1].resolved
            ),
            (Some(true), Some("a1b2c3d"), None)
        );
        assert_eq!(remarks_of(&general)[0].resolved, None);
    }

    #[test]
    fn sums_up_a_pipeline() {
        let run = run_of(
            &json!({ "id": 3026531863u64, "iid": 54657, "ref": "refs/merge-requests/7673/merge", "status": "failed", "source": "merge_request_event", "created_at": "a", "updated_at": "b", "web_url": "https://gitlab.com/p/-/pipelines/3026531863", "name": null }),
        );
        assert_eq!(
            (
                run.id.as_str(),
                run.number,
                run.name.as_str(),
                run.status.as_str(),
                run.finished.as_deref()
            ),
            (
                "3026531863",
                54657,
                "merge_request_event",
                "failure",
                Some("b")
            )
        );
        assert_eq!(
            (run.pull, run.title.as_str(), run.url.as_str()),
            (
                Some(7673),
                "refs/merge-requests/7673/merge",
                "https://gitlab.com/p/-/pipelines/3026531863"
            )
        );
        assert_eq!(
            run_of(&json!({ "status": "pending", "ref": "main" })).pull,
            None
        );
        assert_eq!(run_of(&json!({ "status": "pending" })).finished, None);
        assert_eq!(
            run_of(&json!({ "status": "canceling" })).status,
            "cancelled"
        );
        assert_eq!(
            run_of(&json!({ "status": "success", "name": "Nightly" })).name,
            "Nightly"
        );
    }

    #[test]
    fn keeps_the_project_path_encoded() {
        let repo = Repo {
            account: "a1".into(),
            site: "gitlab.com".into(),
            path: "grp/sub/proj".into(),
        };
        assert_eq!(
            api::url(&repo.site, &repo.request(7)),
            "https://gitlab.com/api/v4/projects/grp%2Fsub%2Fproj/merge_requests/7"
        );
    }

    #[test]
    fn reads_the_site_and_path_of_a_project() {
        let groups = BTreeMap::from([
            ("site".to_string(), "Git.Acme.io".to_string()),
            ("path".to_string(), "team/shop".to_string()),
        ]);
        assert_eq!(
            site_and_path("x", &groups).unwrap(),
            ("git.acme.io".into(), "team/shop".into())
        );
        assert_eq!(
            site_and_path("gitlab.com/grp/sub/proj", &BTreeMap::new()).unwrap(),
            ("gitlab.com".into(), "grp/sub/proj".into())
        );
        assert!(site_and_path("gitlab.com/alone", &BTreeMap::new()).is_err());
    }

    #[test]
    fn counts_the_changed_files() {
        assert_eq!(changed_files(&json!({ "changes_count": "12" })), Some(12));
        assert_eq!(
            changed_files(&json!({ "changes_count": "1000+" })),
            Some(1000)
        );
        assert_eq!(changed_files(&json!({})), None);
    }

    #[test]
    fn asks_for_each_merge_request_once_across_the_three_lists() {
        assert_eq!(
            sorted([vec![1, 2], vec![3], (1..=5).collect()]),
            [1, 2, 3, 4, 5]
        );
        assert_eq!(sorted([vec![], vec![], vec![9, 8]]), [9, 8]);
    }

    #[test]
    fn reads_who_you_are() {
        let user = json!({ "username": "qbriais", "name": "Quentin Briais", "avatar_url": "https://a/q.png" });
        assert_eq!(
            person(&user),
            Person {
                name: "Quentin Briais".into(),
                avatar: "https://a/q.png".into()
            }
        );
    }
}
