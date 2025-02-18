/*
 * Copyright (c) 2023 Stalwart Labs Ltd.
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

use sqlx::{Any, Pool};

use crate::DirectoryOptions;

pub mod config;
pub mod lookup;

pub struct SqlDirectory {
    pool: Pool<Any>,
    mappings: SqlMappings,
    opt: DirectoryOptions,
}

#[derive(Debug)]
pub(crate) struct SqlMappings {
    query_name: String,
    query_members: String,
    query_recipients: String,
    query_emails: String,
    query_domains: String,
    query_verify: String,
    query_expand: String,
    column_name: String,
    column_description: String,
    column_secret: String,
    column_quota: String,
    column_type: String,
}
