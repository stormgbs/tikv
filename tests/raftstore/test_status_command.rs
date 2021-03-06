// Copyright 2016 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// See the License for the specific language governing permissions and
// limitations under the License.

use super::server::*;

#[test]
fn test_region_detail() {
    let count = 5;
    let mut cluster = new_server_cluster(0, count);
    cluster.bootstrap_region().expect("");
    cluster.start();

    let leader = cluster.leader_of_region(1).unwrap();
    let region_detail = cluster.region_detail(1, 1);
    assert!(region_detail.has_region());
    let region = region_detail.get_region();
    assert_eq!(region.get_id(), 1);
    assert!(region.has_start_key());
    assert!(region.get_start_key().is_empty());
    assert!(region.has_end_key());
    assert!(region.get_end_key().is_empty());
    assert_eq!(region.get_store_ids().len(), 5);
    let epoch = region.get_region_epoch();
    assert_eq!(epoch.get_conf_ver(), 1);
    assert_eq!(epoch.get_version(), 1);


    assert!(region_detail.has_leader_store_id());
    assert_eq!(region_detail.get_leader_store_id(), leader);
}
