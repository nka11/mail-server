/*
 * Copyright (c) 2020-2022, Stalwart Labs Ltd.
 *
 * This file is part of Stalwart Mail Server.
 *
 * This program is free software: you can redistribute it and/or modify
 * it under the terms of the GNU Affero General Public License as
 * published by the Free Software Foundation, either version 3 of
 * the License, or (at your option) any later version.
 *
 * This program is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
 * GNU Affero General Public License for more details.
 * in the LICENSE file at the top-level directory of this distribution.
 * You should have received a copy of the GNU Affero General Public License
 * along with this program.  If not, see <http://www.gnu.org/licenses/>.
 *
 * You can be released from the requirements of the AGPLv3 license by
 * purchasing a commercial license. Please contact licensing@stalw.art
 * for more details.
*/

use imap_proto::{
    protocol::{
        list::{
            self, Arguments, Attribute, ChildInfo, ListItem, ReturnOption, SelectionOption, Tag,
        },
        ImapResponse, ProtocolVersion,
    },
    receiver::Request,
    Command, StatusResponse,
};

use tokio::io::AsyncRead;

use crate::core::{Session, SessionData};

impl<T: AsyncRead> Session<T> {
    pub async fn handle_list(&mut self, request: Request<Command>) -> crate::OpResult {
        let command = request.command;
        let is_lsub = command == Command::Lsub;
        match if !is_lsub {
            request.parse_list(self.version)
        } else {
            request.parse_lsub()
        } {
            Ok(arguments) => {
                if !arguments.is_separator_query() {
                    let data = self.state.session_data();
                    let version = self.version;
                    tokio::spawn(async move {
                        data.list(arguments, is_lsub, version).await;
                    });
                    Ok(())
                } else {
                    self.write_bytes(
                        StatusResponse::completed(command)
                            .with_tag(arguments.unwrap_tag())
                            .serialize(
                                list::Response {
                                    is_rev2: self.version.is_rev2(),
                                    is_lsub,
                                    list_items: vec![ListItem {
                                        mailbox_name: String::new(),
                                        attributes: vec![Attribute::NoSelect],
                                        tags: vec![],
                                    }],
                                    status_items: Vec::new(),
                                }
                                .serialize(),
                            ),
                    )
                    .await
                }
            }
            Err(response) => self.write_bytes(response.into_bytes()).await,
        }
    }
}

impl SessionData {
    pub async fn list(&self, arguments: Arguments, is_lsub: bool, version: ProtocolVersion) {
        let (tag, reference_name, mut patterns, selection_options, return_options) = match arguments
        {
            Arguments::Basic {
                tag,
                reference_name,
                mailbox_name,
            } => (
                tag,
                reference_name,
                vec![mailbox_name],
                Vec::new(),
                Vec::new(),
            ),
            Arguments::Extended {
                tag,
                reference_name,
                mailbox_name,
                selection_options,
                return_options,
            } => (
                tag,
                reference_name,
                mailbox_name,
                selection_options,
                return_options,
            ),
        };

        // Refresh mailboxes
        if let Err(err) = self.synchronize_mailboxes(false).await {
            self.write_bytes(err.with_tag(tag).into_bytes()).await;
            return;
        }

        // Process arguments
        let mut filter_subscribed = false;
        let mut filter_special_use = false;
        let mut recursive_match = false;
        let mut include_special_use = version.is_rev2();
        let mut include_subscribed = false;
        let mut include_children = false;
        let mut include_status = None;
        for selection_option in &selection_options {
            match selection_option {
                SelectionOption::Subscribed => {
                    filter_subscribed = true;
                    include_subscribed = true;
                }
                SelectionOption::Remote => (),
                SelectionOption::SpecialUse => {
                    filter_special_use = true;
                    include_special_use = true;
                }
                SelectionOption::RecursiveMatch => {
                    recursive_match = true;
                }
            }
        }
        for return_option in &return_options {
            match return_option {
                ReturnOption::Subscribed => {
                    include_subscribed = true;
                }
                ReturnOption::Children => {
                    include_children = true;
                }
                ReturnOption::Status(status) => {
                    include_status = status.into();
                }
                ReturnOption::SpecialUse => {
                    include_special_use = true;
                }
            }
        }
        if recursive_match && !filter_subscribed {
            self.write_bytes(
                StatusResponse::bad("RECURSIVEMATCH cannot be the only selection option.")
                    .with_tag(tag)
                    .into_bytes(),
            )
            .await;
            return;
        }

        // Append reference name
        if !patterns.is_empty() && !reference_name.is_empty() {
            patterns.iter_mut().for_each(|item| {
                *item = format!("{}{}", reference_name, item);
            })
        }

        let mut list_items = Vec::with_capacity(10);

        // Add "All Mail" folder
        if self.imap.name_all_enable
            && !filter_subscribed
            && matches_pattern(&patterns, &self.imap.name_all)
        {
            list_items.push(ListItem {
                mailbox_name: self.imap.name_all.clone(),
                attributes: vec![Attribute::All, Attribute::NoInferiors],
                tags: vec![],
            });
        }

        // Add mailboxes
        let mut added_shared_folder = false;
        for account in self.mailboxes.lock().iter() {
            if let Some(prefix) = &account.prefix {
                if !added_shared_folder {
                    if !filter_subscribed && matches_pattern(&patterns, &self.imap.name_shared) {
                        list_items.push(ListItem {
                            mailbox_name: self.imap.name_shared.clone(),
                            attributes: if include_children {
                                vec![Attribute::HasChildren, Attribute::NoSelect]
                            } else {
                                vec![Attribute::NoSelect]
                            },
                            tags: vec![],
                        });
                    }
                    added_shared_folder = true;
                }
                if !filter_subscribed && matches_pattern(&patterns, prefix) {
                    list_items.push(ListItem {
                        mailbox_name: prefix.clone(),
                        attributes: if include_children {
                            vec![Attribute::HasChildren, Attribute::NoSelect]
                        } else {
                            vec![Attribute::NoSelect]
                        },
                        tags: vec![],
                    });
                }
            }

            for (mailbox_name, mailbox_id) in &account.mailbox_names {
                if matches_pattern(&patterns, mailbox_name) {
                    let mailbox = account.mailbox_state.get(mailbox_id).unwrap();
                    let mut has_recursive_match = false;
                    if recursive_match {
                        let prefix = format!("{}/", mailbox_name);
                        for (mailbox_name, mailbox_id) in &account.mailbox_names {
                            if mailbox_name.starts_with(&prefix)
                                && account.mailbox_state.get(mailbox_id).unwrap().is_subscribed
                            {
                                has_recursive_match = true;
                                break;
                            }
                        }
                    }
                    if !filter_subscribed || mailbox.is_subscribed || has_recursive_match {
                        let mut attributes = Vec::with_capacity(2);
                        if include_children {
                            attributes.push(if mailbox.has_children {
                                Attribute::HasChildren
                            } else {
                                Attribute::HasNoChildren
                            });
                        }
                        if include_subscribed && mailbox.is_subscribed {
                            attributes.push(Attribute::Subscribed);
                        }
                        if include_special_use {
                            if let Some(special_use) = &mailbox.special_use {
                                attributes.push(*special_use);
                            } else if filter_special_use {
                                continue;
                            }
                        }
                        list_items.push(ListItem {
                            mailbox_name: mailbox_name.clone(),
                            attributes,
                            tags: if !has_recursive_match {
                                vec![]
                            } else {
                                vec![Tag::ChildInfo(vec![ChildInfo::Subscribed])]
                            },
                        });
                    }
                }
            }
        }

        // Add status response
        let mut status_items = Vec::new();
        if let Some(include_status) = include_status {
            for list_item in &list_items {
                match self
                    .status(list_item.mailbox_name.to_string(), include_status)
                    .await
                {
                    Ok(status) => {
                        status_items.push(status);
                    }
                    Err(_) => {
                        tracing::debug!(parent: &self.span, "Failed to get mailbox status.");
                    }
                }
            }
        }

        // Write response
        self.write_bytes(
            StatusResponse::completed(if !is_lsub {
                Command::List
            } else {
                Command::Lsub
            })
            .with_tag(tag)
            .serialize(
                list::Response {
                    is_rev2: version.is_rev2(),
                    is_lsub,
                    list_items,
                    status_items,
                }
                .serialize(),
            ),
        )
        .await;
    }
}

#[allow(clippy::while_let_on_iterator)]
pub fn matches_pattern(patterns: &[String], mailbox_name: &str) -> bool {
    if patterns.is_empty() {
        return true;
    }

    'outer: for pattern in patterns {
        let mut pattern_bytes = pattern.as_bytes().iter().enumerate().peekable();
        let mut mailbox_name = mailbox_name.as_bytes().iter().peekable();

        'inner: while let Some((pos, &ch)) = pattern_bytes.next() {
            if ch == b'%' || ch == b'*' {
                let mut end_pos = pos;
                while let Some((_, &next_ch)) = pattern_bytes.peek() {
                    if next_ch == b'%' || next_ch == b'*' {
                        break;
                    } else {
                        end_pos = pattern_bytes.next().unwrap().0;
                    }
                }
                if end_pos > pos {
                    let match_bytes = &pattern.as_bytes()[pos + 1..end_pos + 1];
                    let mut match_count = 0;
                    let pattern_eof = end_pos == pattern.len() - 1;

                    loop {
                        match mailbox_name.next() {
                            Some(&ch) => {
                                if match_bytes[match_count] == ch {
                                    match_count += 1;
                                    if match_count == match_bytes.len() {
                                        if !pattern_eof {
                                            continue 'inner;
                                        } else if mailbox_name.peek().is_none() {
                                            return true;
                                        } else {
                                            // Match needs to be at the end of the string,
                                            // reset counter.
                                            match_count = 0;
                                        }
                                    }
                                } else if match_count > 0 {
                                    match_count = 0;
                                }
                            }
                            None => continue 'outer,
                        }
                    }
                } else if ch == b'*' || !mailbox_name.any(|&ch| ch == b'/') {
                    return true;
                } else {
                    continue 'outer;
                }
            } else {
                match mailbox_name.next() {
                    Some(&mch) if mch == ch => (),
                    _ => continue 'outer,
                }
            }
        }

        if mailbox_name.next().is_none() {
            return true;
        }
    }

    false
}
