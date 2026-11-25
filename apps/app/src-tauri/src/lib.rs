use tauri::{WebviewUrl, WebviewWindowBuilder};

#[cfg(desktop)]
fn open_outside(url: &str) {
    let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
    let _ = std::process::Command::new(opener).arg(url).spawn();
}

#[cfg(desktop)]
fn origin(url: &str) -> String {
    url.splitn(4, '/').take(3).collect::<Vec<_>>().join("/")
}

#[cfg(desktop)]
fn theme_script(theme: &serde_json::Value) -> String {
    format!("window.corraTheme && window.corraTheme({theme})")
}

#[cfg(desktop)]
fn show_notice(window: tauri::WebviewWindow, n: serde_json::Value) {
    std::thread::spawn(move || {
        let mut body = n["body"].as_str().unwrap_or_default().to_string();
        if let Some(at) = n["resets_at"].as_i64().and_then(chrono::DateTime::from_timestamp_millis) {
            let at = at.with_timezone(&chrono::Local);
            let fmt = if at.date_naive() == chrono::Local::now().date_naive() { "%H:%M" } else { "%a %H:%M" };
            body.push_str(&format!(". Continues at {}.", at.format(fmt)));
        }
        let mut note = notify_rust::Notification::new();
        note.appname("Corra")
            .summary(n["title"].as_str().unwrap_or("Corra"))
            .body(&body);
        #[cfg(target_os = "linux")]
        note.action("default", "Open");
        let Ok(handle) = note.show() else {
            return;
        };
        #[cfg(target_os = "linux")]
        {
            let thread = n["thread"]
                .as_str()
                .filter(|t| n["kind"] != "test" && t.len() == 36 && t.chars().all(|c| c.is_ascii_hexdigit() || c == '-'))
                .map(String::from);
            handle.wait_for_action(|action| {
                if action == "default" {
                    if let Some(t) = &thread {
                        let _ = window.eval(format!("location.hash = '#/t/{t}'"));
                    }
                    let _ = window.unminimize();
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            });
        }
        #[cfg(not(target_os = "linux"))]
        drop(handle);
    });
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default();
    #[cfg(desktop)]
    let builder = builder.plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
        use tauri::Manager;
        if let Some(w) = app.get_webview_window("main") {
            let _ = w.unminimize();
            let _ = w.set_focus();
        }
    }));
    builder
        .setup(|app| {
            #[cfg(desktop)]
            {
                use std::sync::{Arc, Mutex};
                use tauri::Manager;
                use tauri::webview::PageLoadEvent;

                let desktop = tauri::async_runtime::block_on(corra_local::start(corra_local::Options {
                    data: corra_local::default_data_dir(),
                    port: 7811,
                }))?;
                let mut target = desktop.target.clone();
                let first = tauri::async_runtime::block_on(async {
                    loop {
                        if let Some(u) = target.borrow_and_update().url() {
                            return u.to_string();
                        }
                        if target.changed().await.is_err() {
                            return String::new();
                        }
                    }
                });
                let allowed = Arc::new(Mutex::new(vec![origin(desktop.base()), origin(&first)]));
                let theme = desktop.theme.clone();
                let nav = allowed.clone();
                let window = WebviewWindowBuilder::new(app, "main", WebviewUrl::External(first.parse()?))
                    .title("Corra")
                    .inner_size(1360.0, 880.0)
                    .min_inner_size(420.0, 520.0)
                    .on_navigation(move |url| {
                        let ok = nav.lock().unwrap().iter().any(|o| *o == origin(url.as_str()));
                        if !ok {
                            open_outside(url.as_str());
                        }
                        ok
                    })
                    .on_new_window(|url, _| {
                        open_outside(url.as_str());
                        tauri::webview::NewWindowResponse::Deny
                    })
                    .on_page_load(move |w, p| {
                        if p.event() == PageLoadEvent::Finished {
                            let _ = w.eval(theme_script(&theme.borrow()));
                        }
                    })
                    .build()?;
                let follow = window.clone();
                tauri::async_runtime::spawn(async move {
                    while target.changed().await.is_ok() {
                        let next = target.borrow_and_update().url().map(String::from);
                        if let Some(u) = next {
                            {
                                let mut a = allowed.lock().unwrap();
                                let o = origin(&u);
                                if !a.contains(&o) {
                                    a.push(o);
                                }
                            }
                            if let Ok(url) = u.parse() {
                                let _ = follow.navigate(url);
                            }
                        }
                    }
                });
                let mut notices = desktop.notices();
                let pop = window.clone();
                tauri::async_runtime::spawn(async move {
                    loop {
                        match notices.recv().await {
                            Ok(n) => show_notice(pop.clone(), n),
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                            Err(_) => break,
                        }
                    }
                });
                let mut theme = desktop.theme.clone();
                let paint = window.clone();
                tauri::async_runtime::spawn(async move {
                    while theme.changed().await.is_ok() {
                        let script = theme_script(&theme.borrow_and_update());
                        let _ = paint.eval(script);
                    }
                });
                app.manage(desktop);
                #[cfg(target_os = "macos")]
                {
                    use tauri::menu::{MenuBuilder, SubmenuBuilder};
                    let file = SubmenuBuilder::new(app, "Corra").quit().build()?;
                    let edit = SubmenuBuilder::new(app, "Edit").undo().redo().separator().cut().copy().paste().select_all().build()?;
                    app.set_menu(MenuBuilder::new(app).item(&file).item(&edit).build()?)?;
                }
            }
            #[cfg(mobile)]
            {
                WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into())).build()?;
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running corra");
}
