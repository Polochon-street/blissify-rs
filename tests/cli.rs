#[cfg(feature = "integration-tests")]
mod tests {
    use assert_cmd::prelude::*;
    use assert_fs::prelude::*;
    use predicates::prelude::*;
    use std::env;
    use std::os::unix::net::UnixStream;
    use std::path::Path;
    use std::process::{Child, Command, Stdio};
    use std::{thread, time};

    static MPD_CONF: &str = r#"
        music_directory     "MUSIC_DIRECTORY"
        db_file             "DATABASE_FILE"
        bind_to_address     "SOCKET_PATH"
        audio_output {
            type    "null"
            name    "dummy"
        }
    "#;

    struct TestSettings {
        _mpd_conf_file: assert_fs::NamedTempFile,
        socket_file: assert_fs::NamedTempFile,
        _handle: Child,
    }

    fn start_mpd() -> Result<TestSettings, Box<dyn std::error::Error>> {
        let mut data_directory = env::current_dir()?;
        data_directory.push("./data");
        let mpd_conf_file = assert_fs::NamedTempFile::new("mpd.conf")?;
        let socket_file = assert_fs::NamedTempFile::new("socket")?;
        let port: String = format!("{}", 7777);
        let mpd_conf = MPD_CONF
            .replace("MUSIC_DIRECTORY", &data_directory.to_string_lossy())
            .replace(
                "DATABASE_FILE",
                &data_directory.as_path().join("database").to_string_lossy(),
            )
            .replace("PORT", &port)
            .replace("SOCKET_PATH", socket_file.to_str().unwrap());
        mpd_conf_file.write_str(&mpd_conf)?;
        let handle = Command::new("mpd")
            .arg(mpd_conf_file.to_owned().to_str().unwrap())
            .arg("--no-daemon")
            .stderr(Stdio::null())
            .spawn()?;

        Ok(TestSettings {
            _mpd_conf_file: mpd_conf_file,
            socket_file,
            _handle: handle,
        })
    }

    #[test]
    fn test_init_default() -> Result<(), Box<dyn std::error::Error>> {
        env::remove_var("XDG_CONFIG_HOME");
        let mut data_directory = env::current_dir()?;
        data_directory.push("./data");
        let test_settings = start_mpd()?;
        let socket_path = test_settings.socket_file.to_str().unwrap();
        for i in 0..10 {
            match UnixStream::connect(socket_path) {
                Ok(_) => break,
                Err(_) => thread::sleep(time::Duration::from_millis(500)),
            };
            if i == 9 {
                panic!(
                    "Could not start MPD for testing, socket {} still closed",
                    socket_path
                );
            }
        }

        let mut cmd = Command::cargo_bin("blissify")?;
        cmd.arg("init")
            .arg(data_directory)
            .env("MPD_HOST", socket_path);
        cmd.assert().success();
        assert!(Path::new("/tmp/bliss-rs/config.json").exists());
        assert!(Path::new("/tmp/bliss-rs/songs.db").exists());
        Ok(())
    }

    #[test]
    fn test_init_custom_config() -> Result<(), Box<dyn std::error::Error>> {
        let temp_dir = assert_fs::TempDir::new()?;
        assert!(!temp_dir.path().join("test.json").exists());
        assert!(!temp_dir.path().join("test.db").exists());
        let mut data_directory = env::current_dir()?;
        data_directory.push("./data");
        let test_settings = start_mpd()?;
        let socket_path = test_settings.socket_file.to_str().unwrap();
        for i in 0..10 {
            match UnixStream::connect(socket_path) {
                Ok(_) => break,
                Err(_) => thread::sleep(time::Duration::from_millis(500)),
            };
            if i == 9 {
                panic!(
                    "Could not start MPD for testing, socket {} still closed",
                    socket_path
                );
            }
        }

        let mut cmd = Command::cargo_bin("blissify")?;
        cmd.arg("init")
            .arg(data_directory)
            .arg("-d")
            .arg(temp_dir.path().join("test.db"))
            .arg("-c")
            .arg(temp_dir.path().join("test.json"))
            .env("MPD_HOST", socket_path);
        cmd.assert().success();
        assert!(temp_dir.path().join("test.json").exists());
        assert!(temp_dir.path().join("test.db").exists());
        Ok(())
    }

    #[test]
    fn test_list_db_fail() -> Result<(), Box<dyn std::error::Error>> {
        let mut cmd = Command::cargo_bin("blissify")?;

        cmd.arg("list-db").arg("-c").arg("/tmp/nonexisting");
        cmd.assert()
            .failure()
            .stderr(predicate::str::contains("No such file or directory"));

        let mut cmd = Command::cargo_bin("blissify")?;
        cmd.arg("-c").arg("/tmp/nonexisting").arg("list-db");
        cmd.assert()
            .failure()
            .stderr(predicate::str::contains("No such file or directory"));

        Ok(())
    }
}
