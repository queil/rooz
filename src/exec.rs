use std::{io::{stdin, stdout, Write}, time::Duration};
use bollard::{exec::{StartExecResults, ResizeExecOptions, CreateExecOptions}, container::LogOutput, errors::Error};
use futures::{channel::oneshot, StreamExt, Stream};
use nonblock::NonBlockingReader;
use termion::{terminal_size, raw::IntoRawMode};
use tokio::{spawn, io::AsyncWriteExt, time::sleep};
use crate::backend::ExecApi;

async fn collect(
  stream: impl Stream<Item = Result<LogOutput, Error>>,
) -> Result<String, Box<dyn std::error::Error + 'static>> {
  let out = stream
      .map(|x| match x {
          Ok(r) => std::str::from_utf8(r.into_bytes().as_ref())
              .unwrap()
              .to_string(),
          Err(err) => panic!("{}", err),
      })
      .collect::<Vec<_>>()
      .await
      .join("");

  let trimmed = out.trim();
  Ok(trimmed.to_string())
}

impl<'a> ExecApi<'a> {
  async fn start_tty(
      &self,
      exec_id: &str,
      interactive: bool,
  ) -> Result<(), Box<dyn std::error::Error + 'static>> {
      let tty_size = terminal_size()?;
      if let StartExecResults::Attached {
          mut output,
          mut input,
      } = self.client.start_exec(exec_id, None).await?
      {
          let (r, mut s) = oneshot::channel::<bool>();
          let handle = spawn(async move {
              if interactive {
                  let mut stdin = NonBlockingReader::from_fd(stdin()).unwrap();
                  loop {
                      let mut bytes = Vec::new();
                      match stdin.read_available(&mut bytes).ok() {
                          Some(c) if c > 0 => {
                              input.write_all(&bytes).await.ok();
                          }
                          _ => {
                              if let Some(true) = s.try_recv().unwrap() {
                                  break;
                              }
                              sleep(Duration::from_millis(10)).await;
                          }
                      }
                  }
              }
          });

          if interactive {
              match self.client
                  .resize_exec(
                      exec_id,
                      ResizeExecOptions {
                          height: tty_size.1,
                          width: tty_size.0,
                      },
                  )
                  .await
              {
                  Ok(_) => (),
                  Err(err) => println!("Resize exec: {:?}", err),
              };
              println!("{}", termion::clear::All);
          };

          // set stdout in raw mode so we can do tty stuff
          let stdout = stdout();
          let mut stdout = stdout.lock().into_raw_mode()?;
          // pipe docker exec output into stdout
          while let Some(Ok(output)) = output.next().await {
              stdout.write_all(output.into_bytes().as_ref())?;
              stdout.flush()?;
          }

          if interactive {
              r.send(true).ok();
              handle.await?;
          }
      }
      Ok(())
  }

  async fn exec(
      &self,
      reason: &str,
      container_id: &str,
      working_dir: Option<&str>,
      user: Option<&str>,
      cmd: Option<Vec<&str>>,
  ) -> Result<String, Box<dyn std::error::Error + 'static>> {
      #[cfg(not(windows))]
      {
          log::debug!(
              "[{}] exec: {:?} in working dir: {:?}",
              reason,
              cmd,
              working_dir
          );

          Ok(self.client
              .create_exec(
                  &container_id,
                  CreateExecOptions {
                      attach_stdout: Some(true),
                      attach_stderr: Some(true),
                      attach_stdin: Some(true),
                      tty: Some(true),
                      cmd,
                      working_dir,
                      user,
                      ..Default::default()
                  },
              )
              .await?
              .id)
      }
  }

  pub async fn tty(
      &self,
      reason: &str,
      container_id: &str,
      interactive: bool,
      working_dir: Option<&str>,
      user: Option<&str>,
      cmd: Option<Vec<&str>>,
  ) -> Result<(), Box<dyn std::error::Error + 'static>> {
      let exec_id = self.exec(reason, container_id, working_dir, user, cmd).await?;
      self.start_tty(&exec_id, interactive).await
  }

  
  pub async fn output(
      &self,
      reason: &str,
      container_id: &str,
      user: Option<&str>,
      cmd: Option<Vec<&str>>,
  ) -> Result<String, Box<dyn std::error::Error + 'static>> {
      let exec_id = self.exec(reason, container_id, None, user, cmd).await?;
      if let StartExecResults::Attached { output, .. } = self.client.start_exec(&exec_id, None).await?
      {
          collect(output).await
      } else {
          panic!("Could not start exec");
      }
  }
}
