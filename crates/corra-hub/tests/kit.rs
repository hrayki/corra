use std::io::Read;

use corra_hub::db::NewMachine;
use corra_hub::kit::{extract, machine_setups, only};
use corra_proto::MachineStatus;

fn github_tarball(files: &[(&str, &str)]) -> Vec<u8> {
    let gz = flate2::write::GzEncoder::new(vec![], flate2::Compression::default());
    let mut b = tar::Builder::new(gz);
    for (path, text) in files {
        let mut h = tar::Header::new_gnu();
        h.set_size(text.len() as u64);
        h.set_mode(0o755);
        h.set_cksum();
        b.append_data(
            &mut h,
            format!("owner-setup-abc123/{path}"),
            text.as_bytes(),
        )
        .unwrap();
    }
    b.into_inner().unwrap().finish().unwrap()
}

fn paths(tar: &[u8]) -> Vec<(String, String)> {
    let mut a = tar::Archive::new(flate2::read::GzDecoder::new(tar));
    let mut out: Vec<(String, String)> = a
        .entries()
        .unwrap()
        .map(|e| {
            let mut e = e.unwrap();
            let p = e.path().unwrap().to_string_lossy().to_string();
            let mut s = String::new();
            e.read_to_string(&mut s).unwrap();
            (p, s)
        })
        .collect();
    out.sort();
    out
}

#[test]
fn a_machine_gets_shared_and_only_its_own_setups() {
    let (tar, setups) = extract(&github_tarball(&[
        ("README.md", "not the kit"),
        ("corra/shared/AGENTS.md", "rules"),
        ("corra/shared/setup", "#!/bin/sh"),
        ("corra/setups/blog/AGENTS.md", "blog's private rules"),
        ("corra/setups/blog/setup", "#!/bin/sh"),
        ("corra/setups/corra/AGENTS.md", "corra's rules"),
        ("corra/setups/home/AGENTS.md", "home's rules"),
        ("projects/blog/AGENTS.md", "outside the kit"),
    ]))
    .unwrap();
    assert_eq!(setups.len(), 3);

    let blog = paths(&only(&tar, &["blog".into()]).unwrap());
    assert_eq!(
        blog,
        [
            (
                "setups/blog/AGENTS.md".into(),
                "blog's private rules".into()
            ),
            ("setups/blog/setup".into(), "#!/bin/sh".into()),
            ("shared/AGENTS.md".into(), "rules".into()),
            ("shared/setup".into(), "#!/bin/sh".into()),
        ]
    );

    let two = paths(&only(&tar, &["corra".into(), "home".into()]).unwrap());
    let names: Vec<&str> = two.iter().map(|(p, _)| p.as_str()).collect();
    assert_eq!(
        names,
        [
            "setups/corra/AGENTS.md",
            "setups/home/AGENTS.md",
            "shared/AGENTS.md",
            "shared/setup"
        ]
    );

    let none = paths(&only(&tar, &[]).unwrap());
    assert!(none.iter().all(|(p, _)| p.starts_with("shared/")));
}

#[tokio::test]
async fn a_paired_machine_serving_two_projects_gets_both_setups() {
    let dir = tempfile::tempdir().unwrap();
    let built = corra_hub::build(corra_hub::Options {
        data: dir.path().to_path_buf(),
        public_url: "http://127.0.0.1:1".into(),
        dev_no_auth: false,
    })
    .unwrap();
    let hub = built.state.hub;
    let machine = |name: &str| {
        hub.db
            .insert_machine(NewMachine {
                name: name.into(),
                provider: "existing".into(),
                project: None,
                size: "existing".into(),
                token_hash: format!("hash-{name}"),
                provider_ref: None,
                status: MachineStatus::Offline,
            })
            .unwrap()
            .id
    };
    let paired = machine("paired");
    let other = machine("other");
    for (name, setup, on) in [
        ("a", Some("blog"), paired),
        ("b", Some("corra"), paired),
        ("c", None, paired),
        ("d", Some("home"), other),
    ] {
        hub.create_project(corra_hub::hub::NewProject {
            name: name.into(),
            machine: Some(on),
            setup: setup.map(String::from),
            ..Default::default()
        })
        .unwrap();
    }
    assert_eq!(machine_setups(&hub, paired), ["blog", "corra"]);
    assert_eq!(machine_setups(&hub, other), ["home"]);
}
