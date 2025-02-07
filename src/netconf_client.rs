use crate::consts;
use crate::errors::NetconfClientError;
use crate::errors::NetconfClientError::{NetconfError, NetconfResponseIdError, SSHClientError};
use crate::models::{replies::*, requests::*};
use crate::ssh_client::SSHClient;
use quick_xml::se::to_string;
use std::io::{Read, Write};

pub struct NetconfClient {
    ssh_client: SSHClient,
    id: u32,
    session_id: Option<u32>,
}

impl NetconfClient {
    pub fn new(host: &str, port: u16, user: &str, password: &str) -> NetconfClient {
        NetconfClient {
            ssh_client: SSHClient::create(host, port, user, password),
            id: 0,
            session_id: None,
        }
    }

    pub fn get_session_id(&self) -> Option<u32> {
        self.session_id
    }

    pub fn connect(&mut self) -> Result<HelloServer, NetconfClientError> {
        self.ssh_client.connect()?;
        let reply: HelloServer = quick_xml::de::from_str(&self.get_reply()?).unwrap();
        if reply.is_ok() {
            self.session_id = Some(reply.session_id);
        }
        NetconfClient::make_return(reply)
    }

    fn get_reply(&mut self) -> Result<String, NetconfClientError> {
        let mut result = String::new();
        loop {
            let mut buffer = [1u8; 256];
            let bytes_read = self.ssh_client.read(&mut buffer[..])?;
            let s = String::from_utf8_lossy(&buffer[..bytes_read]);
            result.push_str(&s);
            if result.ends_with(consts::MESSAGE_SEPARATOR) {
                break;
            }
            if result.ends_with("##") {
                break;
            }

            dbg!(&result, &bytes_read, &self.ssh_client.eof()?);
            if bytes_read == 0 || self.ssh_client.eof()? {
                return Err(SSHClientError {
                    err: "Buffer is empty, SSH channel read terminated".to_string(),
                });
            }
        }
        Ok(result)
    }

    fn send(&mut self, data: &str) -> Result<(), NetconfClientError> {
        self.ssh_client.write_all(data.as_bytes())?;
        Ok(())
    }

    pub fn send_hello(&mut self) -> Result<(), NetconfClientError> {
        let req = HelloClient {
            xmlns: consts::XMLNS.to_string(),
            capabilities: CapabilitiesClient {
                capabilities: vec![Capability {
                    capability: "urn:ietf:params:netconf:base:1.0".to_string(),
                }],
            },
        };
        let cmd = to_string(&req).unwrap() + consts::MESSAGE_SEPARATOR;
        self.send(&cmd)
    }

    pub fn kill_session(&mut self, session_id: u32) -> Result<KillSessionRsp, NetconfClientError> {
        self.id += 1;
        let req = KillSessionReq {
            message_id: self.id,
            xmlns: consts::XMLNS.to_string(),
            kill_session: KillSession {
                session_id: SessionId { value: session_id },
            },
        };
        let cmd = to_string(&req).unwrap() + consts::MESSAGE_SEPARATOR;
        self.send(&cmd)?;
        let reply: KillSessionRsp = quick_xml::de::from_str(&self.get_reply()?).unwrap();
        self.check_response_message_id(&reply)?;
        NetconfClient::make_return(reply)
    }

    pub fn close_session(&mut self) -> Result<CloseSessionRsp, NetconfClientError> {
        self.id += 1;
        let req = CloseSessionReq {
            message_id: self.id,
            xmlns: consts::XMLNS.to_string(),
            close_session: Default::default(),
        };
        let cmd = to_string(&req).unwrap() + consts::MESSAGE_SEPARATOR;
        self.send(&cmd)?;
        let reply: CloseSessionRsp = quick_xml::de::from_str(&self.get_reply()?).unwrap();
        self.check_response_message_id(&reply)?;
        if reply.is_ok() {
            self.session_id = None;
            self.ssh_client.disconnect()?;
        }
        NetconfClient::make_return(reply)
    }

    pub fn get_config(
        &mut self,
        source: DatastoreType,
        filter: Option<Filter>,
    ) -> Result<GetConfigRsp, NetconfClientError> {
        self.id += 1;
        let mut filter_copy = filter.clone();
        let filter_exists = filter.is_some();
        if filter_exists {
            filter_copy.as_mut().unwrap().data = " ".to_string();
        }
        let req = GetConfigReq {
            message_id: self.id,
            xmlns: consts::XMLNS.to_string(),
            get_config: GetConfig {
                source: Target { target: source },
                filter: filter_copy,
            },
        };
        let mut cmd = to_string(&req).unwrap() + consts::MESSAGE_SEPARATOR;
        if filter_exists {
            cmd.insert_str(cmd.rfind(" </filter>").unwrap(), &filter.unwrap().data);
        }
        self.send(&cmd)?;
        let raw_rsp = self.get_reply()?;
        let mut deserialized_rsp = quick_xml::de::from_str::<GetConfigRsp>(&raw_rsp).unwrap();
        deserialized_rsp.data = Some(NetconfClient::get_data(&raw_rsp).unwrap_or("").to_string());
        self.check_response_message_id(&deserialized_rsp)?;
        NetconfClient::make_return(deserialized_rsp)
    }

    pub fn get(&mut self, filter: Option<Filter>) -> Result<GetRsp, NetconfClientError> {
        self.id += 1;
        let filter_exists = filter.is_some();
        let mut filter_copy = filter.clone();
        if filter_exists {
            filter_copy.as_mut().unwrap().data = " ".to_string();
        }
        let req = GetReq {
            message_id: self.id,
            xmlns: consts::XMLNS.to_string(),
            get: Get {
                filter: filter_copy,
            },
        };

        // hack
        let mut cmd = to_string(&req).unwrap() + consts::MESSAGE_SEPARATOR;
        if filter_exists {
            cmd.insert_str(cmd.rfind(" </filter>").unwrap(), &filter.unwrap().data);
        }
        self.send(&cmd)?;
        dbg!(&cmd);
        let raw_rsp = self.get_reply()?;
        dbg!(&raw_rsp);
        let mut deserialized_rsp = quick_xml::de::from_str::<GetRsp>(&raw_rsp).unwrap();
        deserialized_rsp.data = Some(NetconfClient::get_data(&raw_rsp).unwrap().to_string());
        self.check_response_message_id(&deserialized_rsp)?;
        NetconfClient::make_return(deserialized_rsp)
    }

    pub fn edit_config(
        &mut self,
        source: DatastoreType,
        data: String,
        default_operation: Option<DefaultOperationType>,
        test_option: Option<TestOptionType>,
        error_option: Option<ErrorOptionType>,
    ) -> Result<EditConfigRsp, NetconfClientError> {
        self.id += 1;
        let mut req = EditConfigReq {
            message_id: self.id,
            xmlns: consts::XMLNS.to_string(),
            edit_config: EditConfig {
                target: Target { target: source },
                default_operation: None,
                test_option: None,
                error_option: None,
                config: Data {
                    xmlns_xc: Some("urn:ietf:params:xml:ns:netconf:base:1.0".to_string()),
                    data: " ".to_string(),
                },
            },
        };
        if let Some(default_operation) = default_operation {
            req.edit_config.default_operation = Some(DefaultOperation {
                value: default_operation,
            })
        }
        if let Some(test_option) = test_option {
            req.edit_config.test_option = Some(TestOption { value: test_option })
        }
        if let Some(error_option) = error_option {
            req.edit_config.error_option = Some(ErrorOption {
                value: error_option,
            })
        }
        let mut cmd = to_string(&req).unwrap() + consts::MESSAGE_SEPARATOR;
        // hack
        cmd.insert_str(cmd.rfind(" </config>").unwrap(), &data);
        self.send(&cmd)?;
        let reply: EditConfigRsp = quick_xml::de::from_str(&self.get_reply()?).unwrap();
        self.check_response_message_id(&reply)?;
        NetconfClient::make_return(reply)
    }

    pub fn lock(&mut self, target: DatastoreType) -> Result<LockRsp, NetconfClientError> {
        self.id += 1;
        let model = LockReq {
            xmlns: consts::XMLNS.to_string(),
            message_id: self.id,
            lock: Lock {
                target: Target { target },
            },
        };
        let lock_cmd = to_string(&model).unwrap() + consts::MESSAGE_SEPARATOR;
        self.send(&lock_cmd)?;
        let reply: LockRsp = quick_xml::de::from_str(&self.get_reply()?).unwrap();
        self.check_response_message_id(&reply)?;
        NetconfClient::make_return(reply)
    }

    pub fn unlock(&mut self, target: DatastoreType) -> Result<UnlockRsp, NetconfClientError> {
        self.id += 1;
        let model = UnlockReq {
            xmlns: consts::XMLNS.to_string(),
            message_id: self.id,
            unlock: Unlock {
                target: Target { target },
            },
        };
        let cmd = to_string(&model).unwrap() + consts::MESSAGE_SEPARATOR;
        self.send(&cmd)?;
        let reply: UnlockRsp = quick_xml::de::from_str(&self.get_reply()?).unwrap();
        self.check_response_message_id(&reply)?;
        NetconfClient::make_return(reply)
    }

    pub fn delete_config(
        &mut self,
        target: DatastoreType,
    ) -> Result<DeleteConfigRsp, NetconfClientError> {
        self.id += 1;
        let model = DeleteConfigReq {
            xmlns: consts::XMLNS.to_string(),
            message_id: self.id,
            delete_config: DeleteConfig {
                target: Target { target },
            },
        };
        let cmd = to_string(&model).unwrap() + consts::MESSAGE_SEPARATOR;
        self.send(&cmd)?;
        let reply: DeleteConfigRsp = quick_xml::de::from_str(&self.get_reply()?).unwrap();
        self.check_response_message_id(&reply)?;
        NetconfClient::make_return(reply)
    }

    pub fn discard_changes(&mut self) -> Result<DiscardChangesRsp, NetconfClientError> {
        self.id += 1;
        let model = DiscardChangesReq {
            xmlns: consts::XMLNS.to_string(),
            message_id: self.id,
            discard_changes: Default::default(),
        };
        let cmd = to_string(&model).unwrap() + consts::MESSAGE_SEPARATOR;
        self.send(&cmd)?;
        let reply: DiscardChangesRsp = quick_xml::de::from_str(&self.get_reply()?).unwrap();
        self.check_response_message_id(&reply)?;
        NetconfClient::make_return(reply)
    }

    pub fn commit(&mut self) -> Result<CommitRsp, NetconfClientError> {
        self.id += 1;
        let model = CommitReq {
            xmlns: consts::XMLNS.to_string(),
            message_id: self.id,
            commit: Default::default(),
        };
        let cmd = to_string(&model).unwrap() + consts::MESSAGE_SEPARATOR;
        self.send(&cmd)?;
        let reply: CommitRsp = quick_xml::de::from_str(&self.get_reply()?).unwrap();
        self.check_response_message_id(&reply)?;
        NetconfClient::make_return(reply)
    }

    pub fn copy_config(
        &mut self,
        target: DatastoreType,
        source: CopyConfigSourceType,
    ) -> Result<CopyConfigRsp, NetconfClientError> {
        // TODO
        self.id += 1;
        let model = CopyConfigReq {
            xmlns: consts::XMLNS.to_string(),
            message_id: self.id,
            copy_config: CopyConfig {
                target: Target { target },
                source,
            },
        };
        let cmd = to_string(&model).unwrap() + consts::MESSAGE_SEPARATOR;
        self.send(&cmd)?;
        let reply: CopyConfigRsp = quick_xml::de::from_str(&self.get_reply()?).unwrap();
        self.check_response_message_id(&reply)?;
        NetconfClient::make_return(reply)
    }

    pub fn get_data(text: &str) -> Option<&str> {
        let begin_begin_tag = "<data";
        let end_begin_tag = ">";

        match text.find(begin_begin_tag) {
            Some(value_begin) => {
                let value_end = text[value_begin..]
                    .find(end_begin_tag)
                    .map(|i| i + value_begin)
                    .unwrap();
                let end_element = text.find("</data>").unwrap();
                if value_end + 1 > end_element {
                    return None;
                }
                Some(&text[value_end + 1..end_element])
            }
            None => {
                let begin_begin_tag = "<rpc-error";
                let end_begin_tag = ">";
                let value_begin = text.find(begin_begin_tag).unwrap();
                let value_end = text[value_begin..]
                    .find(end_begin_tag)
                    .map(|i| i + value_begin)
                    .unwrap();
                let end_element = text.find("</rpc-error>").unwrap();
                if value_end + 1 > end_element {
                    return None;
                }
                Some(&text[value_end + 1..end_element])
            }
        }
    }

    fn check_response_message_id<T: RpcRsp>(&self, rsp: &T) -> Result<(), NetconfClientError> {
        let message_id = rsp.get_message_id().expect("no message id");
        if self.id != message_id {
            return Err(NetconfResponseIdError {
                err: format!("request id: {} response id: {}", self.id, message_id),
            });
        }
        Ok(())
    }

    fn make_return<T: RpcRsp>(rsp: T) -> Result<T, NetconfClientError> {
        if rsp.is_ok() {
            Ok(rsp)
        } else {
            Err(NetconfError {
                err: rsp.get_error().unwrap().to_vec(),
            })
        }
    }
}

impl Drop for NetconfClient {
    fn drop(&mut self) {
        if self.session_id.is_some() {
            if let Result::Err(err) = self.close_session() {
                println!("close_session error: {}", err.to_string());
            }
        }
    }
}
