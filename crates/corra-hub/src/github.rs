use anyhow::{Result, anyhow, bail};
use corra_proto::Project;
use serde_json::{Value, json};

use crate::hub::HubRef;

pub struct Gh<'a> {
    hub: &'a HubRef,
    token: String,
    pub repo: String,
}

impl<'a> Gh<'a> {
    pub fn new(hub: &'a HubRef, project: &Project, input: &Value) -> Result<Self> {
        let token = hub.secrets.get("github.token")?.ok_or_else(|| {
            anyhow!("the hub has no GitHub credential yet; add github.token in Settings")
        })?;
        let repos: Vec<&str> = project
            .repos
            .iter()
            .filter_map(|r| r.github.as_deref())
            .collect();
        let repo = match input["repo"].as_str() {
            Some(want) => repos
                .iter()
                .find(|r| {
                    r.eq_ignore_ascii_case(want)
                        || r.rsplit('/')
                            .next()
                            .is_some_and(|n| n.eq_ignore_ascii_case(want))
                })
                .map(|r| r.to_string())
                .ok_or_else(|| {
                    anyhow!(
                        "{want} isn't one of this project's repos ({})",
                        repos.join(", ")
                    )
                })?,
            None => match repos.as_slice() {
                [one] => one.to_string(),
                [] => bail!("this project has no GitHub repos"),
                many => bail!("say which repo: {}", many.join(", ")),
            },
        };
        Ok(Gh { hub, token, repo })
    }

    fn req(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let url = if path.starts_with("https://") {
            path.to_string()
        } else {
            format!(
                "{}/repos/{}{}",
                self.hub.github_api.lock().unwrap(),
                self.repo,
                path
            )
        };
        self.hub
            .http
            .request(method, url)
            .bearer_auth(&self.token)
            .header("user-agent", "corra")
            .header("accept", "application/vnd.github+json")
            .header("x-github-api-version", "2022-11-28")
    }

    async fn send(&self, rb: reqwest::RequestBuilder) -> Result<Value> {
        let r = rb.send().await?;
        let status = r.status();
        let text = r.text().await?;
        let v: Value = serde_json::from_str(&text).unwrap_or(Value::String(text));
        if !status.is_success() {
            let msg = v["message"]
                .as_str()
                .map(String::from)
                .unwrap_or_else(|| v.to_string());
            let errors = v["errors"]
                .as_array()
                .map(|e| format!(" ({})", Value::Array(e.clone())))
                .unwrap_or_default();
            bail!("GitHub answered {status}: {msg}{errors}");
        }
        Ok(v)
    }

    pub async fn get(&self, path: &str) -> Result<Value> {
        self.send(self.req(reqwest::Method::GET, path)).await
    }

    pub async fn post(&self, path: &str, body: Value) -> Result<Value> {
        self.send(self.req(reqwest::Method::POST, path).json(&body))
            .await
    }

    pub async fn patch(&self, path: &str, body: Value) -> Result<Value> {
        self.send(self.req(reqwest::Method::PATCH, path).json(&body))
            .await
    }

    pub async fn put(&self, path: &str, body: Value) -> Result<Value> {
        self.send(self.req(reqwest::Method::PUT, path).json(&body))
            .await
    }

    pub async fn text(&self, path: &str) -> Result<String> {
        let r = self.req(reqwest::Method::GET, path).send().await?;
        if !r.status().is_success() {
            bail!("GitHub answered {}", r.status());
        }
        Ok(r.text().await?)
    }
}

fn short(sha: &str) -> &str {
    &sha[..sha.len().min(12)]
}

pub async fn merge_request(hub: &HubRef, project: &Project, input: &Value) -> Result<Value> {
    let gh = Gh::new(hub, project, input)?;
    let n = num(input, "number")?;
    let pr = gh.get(&format!("/pulls/{n}")).await?;
    if pr["merged_at"].is_string() {
        bail!("pull request #{n} is already merged");
    }
    if pr["state"] != "open" {
        bail!("pull request #{n} is closed");
    }
    let sha = pr["head"]["sha"]
        .as_str()
        .ok_or_else(|| anyhow!("GitHub didn't say which commit pull request #{n} is at"))?;
    Ok(json!({
        "repo": gh.repo,
        "number": n,
        "title": pr["title"],
        "head": pr["head"]["ref"],
        "base": pr["base"]["ref"],
        "sha": sha,
        "method": input["method"].as_str().unwrap_or("squash"),
    }))
}

fn pr_brief(p: &Value) -> Value {
    json!({
        "number": p["number"],
        "title": p["title"],
        "state": if p["merged_at"].is_string() { json!("merged") } else { p["state"].clone() },
        "draft": p["draft"],
        "head": p["head"]["ref"],
        "base": p["base"]["ref"],
        "author": p["user"]["login"],
        "url": p["html_url"],
        "updated_at": p["updated_at"],
    })
}

fn num(input: &Value, key: &str) -> Result<u64> {
    input[key]
        .as_u64()
        .or_else(|| {
            input[key]
                .as_str()
                .and_then(|s| s.trim_start_matches('#').parse().ok())
        })
        .ok_or_else(|| anyhow!("{key} is required"))
}

fn text(input: &Value, key: &str) -> Result<String> {
    input[key]
        .as_str()
        .map(String::from)
        .ok_or_else(|| anyhow!("{key} is required"))
}

pub async fn run(hub: &HubRef, project: &Project, action: &str, input: &Value) -> Result<Value> {
    let gh = Gh::new(hub, project, input)?;
    match action {
        "github.pr.list" => {
            let state = input["state"].as_str().unwrap_or("open");
            let v = gh.get(&format!("/pulls?state={state}&per_page=30")).await?;
            Ok(json!(
                v.as_array()
                    .map(|a| a.iter().map(pr_brief).collect::<Vec<_>>())
                    .unwrap_or_default()
            ))
        }
        "github.pr.view" => {
            let n = num(input, "number")?;
            let p = gh.get(&format!("/pulls/{n}")).await?;
            let sha = p["head"]["sha"].as_str().unwrap_or_default().to_string();
            let checks = checks_summary(&gh, &sha).await.unwrap_or(Value::Null);
            let reviews = gh
                .get(&format!("/pulls/{n}/reviews?per_page=50"))
                .await
                .unwrap_or(json!([]));
            let comments = gh
                .get(&format!("/issues/{n}/comments?per_page=50"))
                .await
                .unwrap_or(json!([]));
            let mut out = pr_brief(&p);
            out["body"] = p["body"].clone();
            out["mergeable"] = p["mergeable"].clone();
            out["mergeable_state"] = p["mergeable_state"].clone();
            out["additions"] = p["additions"].clone();
            out["deletions"] = p["deletions"].clone();
            out["changed_files"] = p["changed_files"].clone();
            out["head_sha"] = json!(sha);
            out["checks"] = checks;
            out["reviews"] = json!(reviews
                .as_array()
                .map(|a| a.iter().map(|r| json!({ "by": r["user"]["login"], "state": r["state"], "body": r["body"] })).collect::<Vec<_>>())
                .unwrap_or_default());
            out["comments"] = json!(comments
                .as_array()
                .map(|a| a.iter().map(|c| json!({ "by": c["user"]["login"], "body": c["body"], "at": c["created_at"] })).collect::<Vec<_>>())
                .unwrap_or_default());
            Ok(out)
        }
        "github.pr.create" => {
            let base = match input["base"].as_str() {
                Some(b) => b.to_string(),
                None => gh.get("").await?["default_branch"]
                    .as_str()
                    .unwrap_or("main")
                    .to_string(),
            };
            let p = gh
                .post(
                    "/pulls",
                    json!({
                        "title": text(input, "title")?,
                        "head": text(input, "head")?,
                        "base": base,
                        "body": input["body"].as_str().unwrap_or_default(),
                        "draft": input["draft"].as_bool().unwrap_or(false),
                    }),
                )
                .await?;
            Ok(pr_brief(&p))
        }
        "github.pr.update" => {
            let n = num(input, "number")?;
            let mut body = serde_json::Map::new();
            for k in ["title", "body", "state", "base"] {
                if let Some(v) = input[k].as_str() {
                    body.insert(k.into(), json!(v));
                }
            }
            if body.is_empty() {
                bail!("nothing to change");
            }
            Ok(pr_brief(
                &gh.patch(&format!("/pulls/{n}"), Value::Object(body))
                    .await?,
            ))
        }
        "github.pr.comment" | "github.issue.comment" => {
            let n = num(input, "number")?;
            let c = gh
                .post(
                    &format!("/issues/{n}/comments"),
                    json!({ "body": text(input, "body")? }),
                )
                .await?;
            Ok(json!({ "url": c["html_url"] }))
        }
        "github.pr.merge" => {
            let n = num(input, "number")?;
            let method = input["method"].as_str().unwrap_or("squash");
            let mut body = json!({ "merge_method": method });
            if let Some(t) = input["title"].as_str() {
                body["commit_title"] = json!(t);
            }
            let approved = input["sha"].as_str();
            if let Some(sha) = approved {
                body["sha"] = json!(sha);
            }
            let r = match gh.put(&format!("/pulls/{n}/merge"), body).await {
                Ok(r) => r,
                Err(e) if approved.is_some() && e.to_string().contains("answered 409") => {
                    bail!(
                        "Not merged: the pull request's branch has new commits since the user allowed the merge of {}. Ask again so they see what would merge now.",
                        short(approved.unwrap())
                    )
                }
                Err(e) => return Err(e),
            };
            Ok(json!({ "merged": r["merged"], "sha": r["sha"], "message": r["message"] }))
        }
        "github.checks" => {
            let r = match input["ref"].as_str() {
                Some(r) => r.to_string(),
                None => bail!("ref is required (a branch, tag or commit)"),
            };
            checks_summary(&gh, &r).await
        }
        "github.run.logs" => {
            let run = num(input, "run_id")?;
            let jobs = gh
                .get(&format!("/actions/runs/{run}/jobs?per_page=50"))
                .await?;
            let mut out = vec![];
            for j in jobs["jobs"].as_array().cloned().unwrap_or_default() {
                let failed = j["conclusion"] == "failure" || j["conclusion"] == "timed_out";
                if !failed && !input["all"].as_bool().unwrap_or(false) {
                    continue;
                }
                let id = j["id"].as_u64().unwrap_or(0);
                let log = gh
                    .text(&format!("/actions/jobs/{id}/logs"))
                    .await
                    .unwrap_or_else(|e| format!("(no log: {e})"));
                let lines: Vec<&str> = log.lines().collect();
                let tail = lines[lines.len().saturating_sub(300)..].join("\n");
                out.push(
                    json!({ "job": j["name"], "conclusion": j["conclusion"], "log_tail": tail }),
                );
            }
            if out.is_empty() {
                return Ok(
                    json!({ "message": "no failed jobs in this run", "jobs": jobs["jobs"].as_array().map(|a| a.iter().map(|j| json!({"name": j["name"], "status": j["status"], "conclusion": j["conclusion"]})).collect::<Vec<_>>()) }),
                );
            }
            Ok(json!(out))
        }
        "github.issue.list" => {
            let state = input["state"].as_str().unwrap_or("open");
            let v = gh
                .get(&format!("/issues?state={state}&per_page=30"))
                .await?;
            Ok(json!(v
                .as_array()
                .map(|a| a
                    .iter()
                    .filter(|i| i["pull_request"].is_null())
                    .map(|i| json!({ "number": i["number"], "title": i["title"], "state": i["state"], "labels": i["labels"].as_array().map(|l| l.iter().map(|x| x["name"].clone()).collect::<Vec<_>>()), "url": i["html_url"] }))
                    .collect::<Vec<_>>())
                .unwrap_or_default()))
        }
        "github.issue.view" => {
            let n = num(input, "number")?;
            let i = gh.get(&format!("/issues/{n}")).await?;
            let comments = gh
                .get(&format!("/issues/{n}/comments?per_page=50"))
                .await
                .unwrap_or(json!([]));
            Ok(json!({
                "number": i["number"], "title": i["title"], "state": i["state"], "body": i["body"], "author": i["user"]["login"], "url": i["html_url"],
                "comments": comments.as_array().map(|a| a.iter().map(|c| json!({ "by": c["user"]["login"], "body": c["body"] })).collect::<Vec<_>>()),
            }))
        }
        "github.issue.create" => {
            let i = gh
                .post("/issues", json!({ "title": text(input, "title")?, "body": input["body"].as_str().unwrap_or_default() }))
                .await?;
            Ok(json!({ "number": i["number"], "url": i["html_url"] }))
        }
        _ => bail!("unknown tool {action}"),
    }
}

fn path_part(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

async fn checks_summary(gh: &Gh<'_>, r: &str) -> Result<Value> {
    if r.is_empty() || r.split('/').any(|p| p == ".." || p == ".") {
        bail!("that isn't a branch, tag or commit");
    }
    let v = gh
        .get(&format!(
            "/commits/{}/check-runs?per_page=100",
            path_part(r)
        ))
        .await?;
    let runs: Vec<Value> = v["check_runs"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|c| {
                    let run_id = c["details_url"]
                        .as_str()
                        .and_then(|u| u.split("/runs/").nth(1))
                        .and_then(|s| s.split('/').next())
                        .and_then(|s| s.parse::<u64>().ok());
                    json!({ "name": c["name"], "status": c["status"], "conclusion": c["conclusion"], "run_id": run_id })
                })
                .collect()
        })
        .unwrap_or_default();
    let failing = runs
        .iter()
        .filter(|c| {
            matches!(
                c["conclusion"].as_str(),
                Some("failure" | "timed_out" | "cancelled")
            )
        })
        .count();
    let pending = runs.iter().filter(|c| c["status"] != "completed").count();
    Ok(json!({ "total": runs.len(), "failing": failing, "pending": pending, "runs": runs }))
}

pub async fn list_repos(hub: &HubRef) -> Result<Value> {
    let token = hub
        .secrets
        .get("github.token")?
        .ok_or_else(|| anyhow::anyhow!("add github.token in Settings → Credentials first"))?;
    let mut out = vec![];
    for page in 1..=5 {
        let v: Value = hub
            .http
            .get(format!(
                "https://api.github.com/user/repos?per_page=100&sort=pushed&page={page}"
            ))
            .bearer_auth(token.trim())
            .header("user-agent", "corra")
            .header("accept", "application/vnd.github+json")
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let Some(list) = v.as_array() else { break };
        for r in list {
            out.push(serde_json::json!({
                "full_name": r["full_name"],
                "private": r["private"],
                "description": r["description"],
                "pushed_at": r["pushed_at"],
            }));
        }
        if list.len() < 100 {
            break;
        }
    }
    Ok(Value::Array(out))
}
