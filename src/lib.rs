use serde_json::{Value, json};
use squadron_extension_api::forge::{self, BranchArgs, Input, NoArgs, NumberArgs};
use squadron_extension_api::{Account, Agent, Extension, host, squadron_extension};

mod api;
mod pulls;
mod reads;

use reads::Repo;

struct GitLab;

fn on_repo<A, T>(
    input: &str,
    work: impl Fn(&Repo, &A) -> Result<T, String>,
) -> Result<String, String>
where
    A: Default + serde::de::DeserializeOwned,
    T: serde::Serialize,
{
    let asked: Input<A> = forge::input(input)?;
    let (site, path) = reads::site_and_path(&asked.repo, &asked.groups)?;
    let group = path.split('/').next().unwrap_or_default().to_lowercase();
    let key = format!("group:{site}/{group}");
    let known = host::state_get(&key);
    let mut last = String::new();
    for account in api::ordered(api::accounts(&site)?, known.clone()) {
        let repo = Repo {
            account: account.clone(),
            site: site.clone(),
            path: path.clone(),
        };
        match work(&repo, &asked.args) {
            Ok(found) => {
                if known.as_deref() != Some(&account) {
                    host::state_set(&key, Some(&account));
                }
                return forge::answer(&found);
            }
            Err(error) if api::denied(&error) => last = error,
            Err(error) => return Err(error),
        }
    }
    Err(last)
}

fn checked(status: u16, body: &str) -> Result<String, String> {
    let user: Value = serde_json::from_str(body).unwrap_or_default();
    match status {
        200 if user["username"].is_string() => {
            let name = user["name"]
                .as_str()
                .filter(|n| !n.is_empty())
                .or(user["username"].as_str())
                .unwrap_or_default();
            Ok(json!({
                "text": { "en": format!("Connected as {name}"), "fr": format!("Connecté en tant que {name}") }
            })
            .to_string())
        }
        200 | 401 | 203 => Err(json!({
            "title": { "en": "GitLab refused this token", "fr": "GitLab a refusé ce jeton" },
            "detail": { "en": "The token may be wrong, expired or revoked, or made on another GitLab.", "fr": "Le jeton est peut-être faux, expiré ou révoqué, ou créé sur un autre GitLab." },
            "field": "token"
        })
        .to_string()),
        403 => Err(json!({
            "title": { "en": "This token lacks the api scope", "fr": "Ce jeton n’a pas le scope api" },
            "detail": { "en": "Create a token with the api scope.", "fr": "Créez un jeton avec le scope api." },
            "field": "token"
        })
        .to_string()),
        status => Err(format!("GitLab answered {status}.")),
    }
}

impl Extension for GitLab {
    fn on_event(name: String, _agent: Option<Agent>, _data: String) {
        if name == "accounts.changed" {
            for account in host::accounts() {
                host::state_set(&reads::username_key(&account.id), None);
            }
        }
    }

    fn check_account(account: Account) -> Result<String, String> {
        let site = api::site_of(&account.fields);
        let me = api::get(&account.id, &site, "/user")?;
        checked(me.status, &me.body)
    }

    fn forge(call: String, input: String) -> Result<String, String> {
        match call.as_str() {
            "pull" => on_repo(&input, |repo, args: &BranchArgs| {
                reads::pull(repo, &args.branch)
            }),
            "numbered" => on_repo(&input, |repo, args: &NumberArgs| {
                reads::numbered(repo, args.number)
            }),
            "lists" => on_repo(&input, |repo, _: &NoArgs| reads::lists(repo)),
            "remarks" => on_repo(&input, |repo, args: &NumberArgs| {
                reads::remarks(repo, args.number)
            }),
            "me" => on_repo(&input, |repo, _: &NoArgs| reads::me(repo)),
            "runs" => on_repo(&input, |repo, _: &NoArgs| reads::runs(repo)),
            _ => Err(forge::unsupported(&call)),
        }
    }
}

squadron_extension!(GitLab);

#[cfg(test)]
mod tests {
    use super::*;
    use regex::Regex;

    fn manifest() -> Value {
        serde_json::from_str(include_str!("../extension.json")).unwrap()
    }

    fn patterns(key: &str, hosts: &[&str]) -> Vec<Regex> {
        let hosts: Vec<String> = hosts.iter().map(|h| regex::escape(h)).collect();
        manifest()["contributes"]["forge"][key]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p.as_str().unwrap())
            .filter(|p| !p.contains("{account}") || !hosts.is_empty())
            .map(|p| {
                Regex::new(&p.replace("{account}", &format!("(?:{})", hosts.join("|")))).unwrap()
            })
            .collect()
    }

    fn repo(found: &regex::Captures) -> String {
        format!("{}/{}", &found["site"], &found["path"])
    }

    #[test]
    fn recognizes_gitlab_remotes() {
        let matched = |remote: &str, hosts: &[&str]| {
            patterns("remotes", hosts)
                .iter()
                .find_map(|r| r.captures(remote).map(|c| repo(&c)))
        };
        assert_eq!(
            matched("gitlab.com/gitlab-org/gitlab-runner", &[]).as_deref(),
            Some("gitlab.com/gitlab-org/gitlab-runner")
        );
        assert_eq!(
            matched("gitlab.com/grp/sub/proj", &[]).as_deref(),
            Some("gitlab.com/grp/sub/proj")
        );
        assert_eq!(matched("gitlab.com/alone", &[]), None);
        assert_eq!(matched("git.acme.io/team/shop", &[]), None);
        assert_eq!(
            matched("git.acme.io/team/shop", &["git.acme.io"]).as_deref(),
            Some("git.acme.io/team/shop")
        );
        assert_eq!(matched("gitxacme.io/team/shop", &["git.acme.io"]), None);
        assert_eq!(
            matched("github.com/Jgrasp/squadron", &["git.acme.io"]),
            None
        );
        assert_eq!(matched("notgitlab.com/a/b", &[]), None);
    }

    #[test]
    fn recognizes_merge_request_links() {
        let matched = |url: &str, hosts: &[&str]| {
            patterns("links", hosts)
                .iter()
                .map(|r| Regex::new(&format!("^{}([/?#].*)?$", r.as_str())).unwrap())
                .find_map(|r| r.captures(url).map(|c| (repo(&c), c["number"].to_string())))
        };
        assert_eq!(
            matched(
                "https://gitlab.com/gitlab-org/gitlab-runner/-/merge_requests/7767",
                &[]
            ),
            Some(("gitlab.com/gitlab-org/gitlab-runner".into(), "7767".into()))
        );
        assert_eq!(
            matched(
                "https://gitlab.com/grp/sub/proj/-/merge_requests/3/diffs",
                &[]
            ),
            Some(("gitlab.com/grp/sub/proj".into(), "3".into()))
        );
        assert_eq!(
            matched(
                "https://git.acme.io/team/shop/-/merge_requests/12",
                &["git.acme.io"]
            ),
            Some(("git.acme.io/team/shop".into(), "12".into()))
        );
        assert_eq!(
            matched("https://git.acme.io/team/shop/-/merge_requests/12", &[]),
            None
        );
        assert_eq!(
            matched("https://gitlab.com/team/shop/-/issues/12", &[]),
            None
        );
        assert_eq!(
            matched("https://github.com/agence/orbit/pull/2108", &[]),
            None
        );
    }

    #[test]
    fn labels_every_text_in_english_and_french() {
        let found = manifest();
        let account = &found["contributes"]["accounts"];
        let mut texts = vec![&found["name"], &found["description"], &account["label"]];
        for field in account["fields"].as_array().unwrap() {
            texts.push(&field["label"]);
            texts.push(&field["hint"]);
        }
        for text in texts {
            assert!(text["en"].is_string() && text["fr"].is_string(), "{text}");
        }
    }

    #[test]
    fn tells_whether_an_account_works() {
        let ok: Value = serde_json::from_str(
            &checked(200, r#"{"username":"qbriais","name":"Quentin Briais"}"#).unwrap(),
        )
        .unwrap();
        assert_eq!(ok["text"]["en"], "Connected as Quentin Briais");
        let bare: Value =
            serde_json::from_str(&checked(200, r#"{"username":"qbriais","name":""}"#).unwrap())
                .unwrap();
        assert_eq!(bare["text"]["en"], "Connected as qbriais");
        for status in [200, 401] {
            let refused: Value = serde_json::from_str(&checked(status, "{}").unwrap_err()).unwrap();
            assert_eq!(refused["field"], "token");
        }
        let scoped: Value = serde_json::from_str(&checked(403, "").unwrap_err()).unwrap();
        assert_eq!(scoped["title"]["en"], "This token lacks the api scope");
        assert_eq!(checked(500, "").unwrap_err(), "GitLab answered 500.");
    }
}
