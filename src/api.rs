use serde_json::{Value, json};
use squadron_extension_api::forge;
use squadron_extension_api::{Request, Response, host};

pub fn site_of(fields: &[(String, String)]) -> String {
    fields
        .iter()
        .find(|(k, _)| k == "site")
        .map(|(_, v)| v.to_lowercase())
        .unwrap_or_default()
}

pub fn accounts(site: &str) -> Result<Vec<String>, String> {
    let found: Vec<String> = host::accounts()
        .into_iter()
        .filter(|a| site_of(&a.fields) == site.to_lowercase())
        .map(|a| a.id)
        .collect();
    if found.is_empty() {
        return Err(forge::no_account(json!({
            "en": format!("Add a GitLab account for {site} in Settings › Extensions › GitLab."),
            "fr": format!("Ajoutez un compte GitLab pour {site} dans Réglages › Extensions › GitLab."),
        })));
    }
    Ok(found)
}

pub fn ordered(mut accounts: Vec<String>, known: Option<String>) -> Vec<String> {
    if let Some(i) = known.and_then(|k| accounts.iter().position(|a| *a == k)) {
        let first = accounts.remove(i);
        accounts.insert(0, first);
    }
    accounts
}

pub fn denied(error: &str) -> bool {
    ["\"code\":\"account\"", "answered 403", "answered 404"]
        .iter()
        .any(|mark| error.contains(mark))
}

pub fn encode(text: &str) -> String {
    text.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

pub fn query(path: &str, query: &[(&str, &str)]) -> String {
    if query.is_empty() {
        return path.into();
    }
    let query: Vec<String> = query
        .iter()
        .map(|(k, v)| format!("{}={}", encode(k), encode(v)))
        .collect();
    format!("{path}?{}", query.join("&"))
}

pub fn url(site: &str, path: &str) -> String {
    format!("https://{site}/api/v4{path}")
}

pub fn request(method: &str, url: String, account: &str, body: Option<String>) -> Request {
    let mut headers = vec![
        ("PRIVATE-TOKEN".into(), format!("{{{{{account}.token}}}}")),
        ("Accept".into(), "application/json".into()),
    ];
    if body.is_some() {
        headers.push(("Content-Type".into(), "application/json".into()));
    }
    Request {
        method: method.into(),
        url,
        headers,
        body,
    }
}

pub fn send(
    account: &str,
    site: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Result<Value, String> {
    let request = request(
        method,
        url(site, path),
        account,
        body.map(|b| b.to_string()),
    );
    answer(host::fetch(&request)?)
}

pub fn get(account: &str, site: &str, path: &str) -> Result<Response, String> {
    host::fetch(&request("GET", url(site, path), account, None))
}

// shortcut: one request after the other, until the API's host::fetch_many sends them at once.
fn fetch_many(requests: &[Request]) -> Vec<Result<Response, String>> {
    requests.iter().map(host::fetch).collect()
}

pub fn each(account: &str, site: &str, paths: &[String]) -> Vec<Result<Value, String>> {
    let requests: Vec<Request> = paths
        .iter()
        .map(|path| request("GET", url(site, path), account, None))
        .collect();
    fetch_many(&requests)
        .into_iter()
        .map(|found| found.and_then(answer))
        .collect()
}

pub fn refused() -> String {
    forge::no_account(json!({
        "en": "GitLab refused the token: add a new one in Settings › Extensions › GitLab.",
        "fr": "GitLab a refusé le jeton : ajoutez-en un nouveau dans Réglages › Extensions › GitLab.",
    }))
}

pub fn failure(status: u16, body: &Value, raw: &str) -> String {
    let reason = [&body["error"]["message"], &body["message"], &body["error"]]
        .into_iter()
        .find_map(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| raw.chars().take(200).collect());
    format!("GitLab answered {status}: {reason}")
}

pub fn answer(response: Response) -> Result<Value, String> {
    if response.status == 401 || response.status == 203 {
        return Err(refused());
    }
    let body: Value = serde_json::from_str(&response.body).unwrap_or(Value::Null);
    if !(200..300).contains(&response.status) {
        return Err(failure(response.status, &body, &response.body));
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tries_the_account_that_reached_the_group_first() {
        let all = vec!["a1".to_string(), "a2".into(), "a3".into()];
        assert_eq!(ordered(all.clone(), Some("a2".into())), ["a2", "a1", "a3"]);
        assert_eq!(ordered(all.clone(), Some("gone".into())), all);
        assert_eq!(ordered(all.clone(), None), all);
    }

    #[test]
    fn tells_a_refused_project_from_another_failure() {
        assert!(denied(&refused()));
        assert!(denied("GitLab answered 404: 404 Project Not Found"));
        assert!(denied("GitLab answered 403: 403 Forbidden"));
        assert!(!denied("GitLab answered 400: Bad request"));
    }

    #[test]
    fn reads_the_site_of_an_account() {
        let fields = vec![
            ("site".to_string(), "Git.Acme.io".to_string()),
            ("token".to_string(), String::new()),
        ];
        assert_eq!(site_of(&fields), "git.acme.io");
        assert_eq!(site_of(&[]), "");
    }

    #[test]
    fn encodes_queries_gitlab_reads() {
        assert_eq!(
            url(
                "gitlab.com",
                &query(
                    "/projects/grp%2Fsub%2Fproj/merge_requests",
                    &[("source_branch", "feat/x y"), ("per_page", "1")]
                )
            ),
            "https://gitlab.com/api/v4/projects/grp%2Fsub%2Fproj/merge_requests?source_branch=feat%2Fx%20y&per_page=1"
        );
        assert_eq!(
            url("git.acme.io", &query("/user", &[])),
            "https://git.acme.io/api/v4/user"
        );
    }

    #[test]
    fn explains_why_gitlab_refused() {
        let body = json!({ "error": "insufficient_scope", "error_description": "The request requires higher privileges than provided by the access token." });
        assert_eq!(
            failure(403, &body, ""),
            "GitLab answered 403: insufficient_scope"
        );
        let body = json!({ "message": "404 Project Not Found" });
        assert_eq!(
            failure(404, &body, ""),
            "GitLab answered 404: 404 Project Not Found"
        );
        assert_eq!(
            failure(502, &Value::Null, "Bad gateway"),
            "GitLab answered 502: Bad gateway"
        );
        let refused: Value = serde_json::from_str(&refused()).unwrap();
        assert_eq!(refused["code"], "account");
    }

    #[test]
    fn asks_squadron_for_the_token_of_the_account() {
        let sent = request(
            "POST",
            url("gitlab.com", "/user"),
            "acc1",
            Some("{}".into()),
        );
        assert_eq!(
            sent.headers[0],
            ("PRIVATE-TOKEN".to_string(), "{{acc1.token}}".to_string())
        );
        assert!(
            sent.headers
                .iter()
                .any(|(k, v)| k == "Content-Type" && v == "application/json")
        );
    }
}
