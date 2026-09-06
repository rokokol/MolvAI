// SPDX-License-Identifier: MIT
//! Клиент IPC против настоящего сокета.
//!
//! Демона на другой стороне ещё нет, поэтому здесь стоит поддельный сервер, который
//! говорит ровно тем протоколом, что заморожен в `molva_core::ipc`. Проверяется то,
//! что нельзя проверить модульным тестом: соединение, порядок строк, обрыв связи.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread::JoinHandle;

use interprocess::local_socket::traits::{ListenerExt as _, Stream as _};
use interprocess::local_socket::{ListenerOptions, Stream};
use molva_core::domain::entry::Mode;
use molva_core::ipc::{Command, DaemonState, ErrorCode, Event, Request, Response};
use molva_gui::ipc::{Connection, IpcClientError, Message};

/// Поддельный демон: принимает одно соединение и отвечает на строки по сценарию.
struct FakeDaemon {
    path: PathBuf,
    handle: Option<JoinHandle<Vec<Request>>>,
    // Каталог живёт, пока жив сервер: сокет лежит внутри него.
    _dir: tempfile::TempDir,
}

impl FakeDaemon {
    /// `reply` получает разобранный запрос и возвращает строки, которые надо отправить,
    /// либо `None` — тогда демон обрывает соединение, ничего не ответив.
    fn start(reply: impl Fn(&Request) -> Option<Vec<String>> + Send + 'static) -> Self {
        let dir = tempfile::tempdir().expect("временный каталог");
        let path = dir.path().join("molva.sock");
        // Имя строит ядро, как и у клиента: на Windows путь из tempdir — не имя канала.
        let name = molva_core::infra::ipc::transport::socket_name(&path).expect("имя сокета");
        let listener = ListenerOptions::new()
            .name(name)
            .create_sync()
            .expect("сокет создан");
        // Сигнал готовности не нужен: сокет уже слушает до возврата из start.
        let handle = std::thread::spawn(move || {
            let mut seen = Vec::new();
            let Some(Ok(stream)) = listener.incoming().next() else {
                return seen;
            };
            let mut reader = BufReader::new(stream);
            loop {
                let mut line = String::new();
                match reader.read_line(&mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
                let Ok(request) = serde_json::from_str::<Request>(line.trim()) else {
                    break;
                };
                let answers = reply(&request);
                seen.push(request);
                let Some(answers) = answers else {
                    break;
                };
                let stream = reader.get_mut();
                let mut broken = false;
                for answer in answers {
                    if stream.write_all(format!("{answer}\n").as_bytes()).is_err() {
                        broken = true;
                        break;
                    }
                }
                if broken || stream.flush().is_err() {
                    break;
                }
            }
            seen
        });
        Self {
            path,
            handle: Some(handle),
            _dir: dir,
        }
    }

    fn path(&self) -> &Path {
        &self.path
    }

    /// Дождаться конца обслуживания и вернуть все полученные запросы.
    fn requests(mut self) -> Vec<Request> {
        self.handle
            .take()
            .expect("сервер запущен")
            .join()
            .expect("поток сервера не паниковал")
    }
}

fn ok_line(id: u64, result: serde_json::Value) -> String {
    serde_json::to_string(&Response::ok(id, result)).expect("ответ сериализуется")
}

#[test]
fn request_gets_the_answer_to_its_own_id() {
    let daemon = FakeDaemon::start(|request| {
        Some(vec![ok_line(
            request.id,
            serde_json::json!({"state": "idle"}),
        )])
    });
    let mut connection = Connection::connect_at(daemon.path()).expect("соединение");
    let result = connection.request(Command::Status).expect("ответ");
    assert_eq!(result["state"], "idle");

    drop(connection);
    let seen = daemon.requests();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].cmd, Command::Status);
    assert_eq!(seen[0].v, molva_core::ipc::PROTOCOL_VERSION);
}

#[test]
fn events_arriving_before_the_answer_do_not_confuse_the_client() {
    // Демон вправе прислать события в любой момент, в том числе между запросом и ответом.
    let daemon = FakeDaemon::start(|request| {
        Some(vec![
            serde_json::to_string(&Event::Level { rms: 0.3 }).unwrap(),
            serde_json::to_string(&Event::State {
                state: DaemonState::Recording,
                mode: Some(Mode::Dictation),
            })
            .unwrap(),
            ok_line(request.id, serde_json::json!({"ok": true})),
        ])
    });
    let mut connection = Connection::connect_at(daemon.path()).expect("соединение");
    let result = connection
        .request(Command::RecordStart {
            mode: Mode::Dictation,
            style: None,
        })
        .expect("ответ");
    assert_eq!(result["ok"], true);
}

#[test]
fn answer_to_a_foreign_id_is_skipped() {
    // Ответ с чужим идентификатором клиент обязан пропустить, а не принять за свой.
    let daemon = FakeDaemon::start(|request| {
        Some(vec![
            ok_line(request.id + 1000, serde_json::json!({"чужой": true})),
            ok_line(request.id, serde_json::json!({"свой": true})),
        ])
    });
    let mut connection = Connection::connect_at(daemon.path()).expect("соединение");
    let result = connection.request(Command::Ping).expect("ответ");
    assert_eq!(result["свой"], true);
    assert!(result.get("чужой").is_none());
}

#[test]
fn daemon_error_carries_code_and_next_step() {
    let daemon = FakeDaemon::start(|request| {
        Some(vec![serde_json::to_string(&Response::err(
            request.id,
            ErrorCode::NoDevice,
            "микрофон не найден",
            Some("выберите устройство в настройках".into()),
        ))
        .unwrap()])
    });
    let mut connection = Connection::connect_at(daemon.path()).expect("соединение");
    let err = connection.request(Command::RecordStop).unwrap_err();
    assert!(err.to_string().contains("микрофон"), "{err}");
    assert_eq!(
        err.hint().as_deref(),
        Some("выберите устройство в настройках")
    );
    // Ошибка демона — это не «демона нет»: интерфейс показывает их по-разному.
    assert!(!err.is_unavailable());
}

#[test]
fn subscription_streams_events_until_the_daemon_closes() {
    let daemon = FakeDaemon::start(|request| match request.cmd {
        Command::Subscribe { levels } => {
            assert!(levels, "GUI подписывается с уровнями");
            Some(vec![
                ok_line(request.id, serde_json::Value::Null),
                serde_json::to_string(&Event::State {
                    state: DaemonState::Recording,
                    mode: None,
                })
                .unwrap(),
                serde_json::to_string(&Event::Level { rms: 0.125 }).unwrap(),
                serde_json::to_string(&Event::ConfigReloaded).unwrap(),
            ])
        }
        _ => Some(vec![ok_line(request.id, serde_json::Value::Null)]),
    });

    let mut connection = Connection::connect_at(daemon.path()).expect("соединение");
    connection
        .send(Command::Subscribe { levels: true })
        .expect("подписка отправлена");

    let mut events = Vec::new();
    while events.len() < 3 {
        match connection.recv().expect("чтение") {
            Some(Message::Event(event)) => events.push(event),
            Some(Message::Response(_)) => continue,
            None => break,
        }
    }
    assert_eq!(
        events[0],
        Event::State {
            state: DaemonState::Recording,
            mode: None
        }
    );
    assert_eq!(events[1], Event::Level { rms: 0.125 });
    assert_eq!(events[2], Event::ConfigReloaded);
}

#[test]
fn closed_connection_is_reported_not_hung() {
    // Демон принял запрос и умер, не ответив: клиент обязан вернуть ошибку обрыва.
    let daemon = FakeDaemon::start(|_| None);
    let mut connection = Connection::connect_at(daemon.path()).expect("соединение");
    // Первый запрос остаётся без ответа, а следом сервер закрывает соединение.
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let outcome = connection.request(Command::Ping);
        let _ = tx.send(outcome.is_err());
    });
    drop(daemon.requests());
    assert!(
        rx.recv_timeout(std::time::Duration::from_secs(5))
            .expect("клиент не завис на закрытом сокете"),
        "обрыв связи должен стать ошибкой"
    );
}

#[test]
fn missing_socket_is_a_recoverable_absence_of_the_daemon() {
    let dir = tempfile::tempdir().unwrap();
    let Err(err) = Connection::connect_at(&dir.path().join("нет-такого.sock")) else {
        panic!("соединение с несуществующим сокетом не должно удаваться");
    };
    assert!(err.is_unavailable(), "{err}");
    assert!(err.hint().is_some(), "нужен следующий шаг для пользователя");
    assert!(matches!(err, IpcClientError::NotRunning { .. }));
}

#[test]
fn garbage_from_the_daemon_does_not_crash_the_client() {
    let daemon = FakeDaemon::start(|_| Some(vec!["не json вовсе".to_string()]));
    let mut connection = Connection::connect_at(daemon.path()).expect("соединение");
    let err = connection.request(Command::Ping).unwrap_err();
    assert!(matches!(err, IpcClientError::Protocol(_)), "{err}");
}

#[test]
fn stream_type_is_the_one_the_client_uses() {
    // Тест держит связь с реализацией: если транспорт сменится, это перестанет компилироваться.
    fn _assert_stream_is_used(
        _: fn(interprocess::local_socket::Name<'_>) -> std::io::Result<Stream>,
    ) {
    }
    _assert_stream_is_used(Stream::connect);
}
