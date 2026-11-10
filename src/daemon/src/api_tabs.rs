//! Agent tab lifecycle uses the same creation/close paths as graphical clients.
use super::{failure, record, success};
use crate::service::Service;
use std::io;
use worminal_protocol::worp::{Reply, Request};
use worminal_workspace::launch::Launch;

impl Service {
    pub(crate) fn open_detached(&mut self, launch: &Launch, now: i64) -> io::Result<Reply> {
        let index = match self.open_terminal(launch, None) {
            Ok(index) => index,
            Err(error) => return Ok(failure(&format!("Terminal could not be opened: {error}"))),
        };
        self.created_reply(index, now)
    }

    pub(crate) fn created_reply(&mut self, index: usize, now: i64) -> io::Result<Reply> {
        let id = self.tabs[index].id;
        let metadata = record(&mut self.tabs[index], now);
        self.publish(now)?;
        Ok(success(format!(
            "{{\"ok\":true,\"id\":{id},\"created\":true,\"tab\":{metadata}}}"
        )))
    }

    pub(super) fn tab_operation(
        &mut self,
        request: &Request,
        now: i64,
    ) -> io::Result<Option<Reply>> {
        let index = match request {
            Request::OpenTerminal { options } => {
                let launch = Launch::from_options(options);
                match self.open_terminal(&launch, None) {
                    Ok(index) => index,
                    Err(error) => {
                        return Ok(Some(failure(&format!(
                            "Terminal could not be opened: {error}"
                        ))))
                    }
                }
            }
            Request::OpenBrowser { url } => {
                let id = self.open_browser(url.as_bytes().to_vec())?;
                self.index(id).unwrap()
            }
            Request::Close { tab } => {
                let Some(index) = self.index(*tab) else {
                    return Ok(Some(failure("Tab not found; run worminald api list.")));
                };
                self.close(index);
                self.publish(now)?;
                return Ok(Some(success(format!(
                    "{{\"ok\":true,\"id\":{tab},\"closed\":true}}"
                ))));
            }
            _ => return Ok(None),
        };
        Ok(Some(self.created_reply(index, now)?))
    }
}
