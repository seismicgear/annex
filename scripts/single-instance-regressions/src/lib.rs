//! Real session-bus regression for the API used by the native Linux adapter.
//! This is a transport component test, not native application acceptance.
#[cfg(test)]
mod tests {
    use std::{sync::mpsc, time::Duration};
    use zbus::{block_on, connection::Builder, interface, Connection};

    struct Receiver(mpsc::Sender<(Vec<String>, String)>);
    #[interface(name = "org.SingleInstance.DBus")]
    impl Receiver {
        fn execute_callback(&mut self, argv: Vec<String>, cwd: String) {
            self.0.send((argv, cwd)).unwrap();
        }
    }

    #[test]
    fn idle_owner_receives_arguments_without_replacement_and_releases_its_name() {
        assert!(
            std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_some(),
            "run under dbus-run-session"
        );
        let name = format!(
            "com.annex.Acceptance.P{}.SingleInstance",
            std::process::id()
        );
        let path = "/com/annex/Acceptance/SingleInstance";
        let (send, receive) = mpsc::channel();
        let owner = block_on(
            Builder::session()
                .name(name.as_str())
                .replace_existing_names(false)
                .allow_name_replacements(false)
                .serve_at(path, Receiver(send))
                .build(),
        )
        .unwrap();
        // The owning block_on call has returned. The library's helper runtime
        // must keep dispatching calls while the desktop event loop runs.
        let occupied = block_on(
            Builder::session()
                .name(name.as_str())
                .replace_existing_names(false)
                .allow_name_replacements(false)
                .build(),
        );
        assert!(matches!(occupied, Err(zbus::Error::NameTaken)));
        let argv = vec![
            "/opt/Annex/annex-desktop".to_owned(),
            "annex://invite/test-%E2%9C%93".to_owned(),
        ];
        let cwd = "/tmp/Annex acceptance".to_owned();
        let expected = (argv.clone(), cwd.clone());
        let target = name.clone();
        std::thread::spawn(move || {
            block_on(async move {
                let client = Connection::session().await.unwrap();
                client
                    .call_method(
                        Some(target.as_str()),
                        path,
                        Some("org.SingleInstance.DBus"),
                        "ExecuteCallback",
                        &(argv, cwd),
                    )
                    .await
                    .unwrap();
            })
        })
        .join()
        .unwrap();
        assert_eq!(
            receive.recv_timeout(Duration::from_secs(5)).unwrap(),
            expected
        );
        assert!(block_on(owner.release_name(name.as_str())).unwrap());
        let replacement = block_on(
            Builder::session()
                .name(name.as_str())
                .replace_existing_names(false)
                .allow_name_replacements(false)
                .build(),
        )
        .unwrap();
        assert!(block_on(replacement.release_name(name.as_str())).unwrap());
    }
}
