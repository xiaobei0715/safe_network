// Copyright 2023 MaidSafe.net limited.
//
// This SAFE Network Software is licensed to you under The General Public License (GPL), version 3.
// Unless required by applicable law or agreed to in writing, the SAFE Network Software distributed
// under the GPL Licence is distributed on an "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY
// KIND, either express or implied. Please review the Licences for the specific language governing
// permissions and limitations relating to use of the SAFE Network Software.

use libp2p::{kad::KBucketKey, PeerId};
use rayon::iter::{IntoParallelIterator, ParallelIterator};
use sn_protocol::NetworkAddress;
use std::{
    collections::{hash_map::Entry, HashMap, HashSet, VecDeque},
    time::Instant,
};

// The number of PeerId to generate when starting an instance of NetworkDiscovery
const INITIAL_GENERATION_ATTEMPTS: usize = 10_000;
// The number of PeerId to generate during each invocation to refresh our candidates
const GENERATION_ATTEMPTS: usize = 1_000;
// The max number of PeerId to keep per bucket
const MAX_PEERS_PER_BUCKET: usize = 5;

/// Keep track of NetworkAddresses belonging to every bucket (if we can generate them with reasonable effort)
/// which we can then query using Kad::GetClosestPeers to effectively fill our RT.
#[derive(Debug, Clone)]
pub(crate) struct NetworkDiscovery {
    self_key: KBucketKey<PeerId>,
    candidates: HashMap<u32, VecDeque<NetworkAddress>>,
}

impl NetworkDiscovery {
    /// Create a new instance of NetworkDiscovery and tries to populate each bucket with random peers.
    pub(crate) fn new(self_peer_id: &PeerId) -> Self {
        let start = Instant::now();
        let self_key = KBucketKey::from(*self_peer_id);
        let candidates_vec = Self::generate_candidates(&self_key, INITIAL_GENERATION_ATTEMPTS);

        let mut candidates: HashMap<u32, VecDeque<NetworkAddress>> = HashMap::new();
        for (ilog2, candidate) in candidates_vec {
            match candidates.entry(ilog2) {
                Entry::Occupied(mut entry) => {
                    let entry = entry.get_mut();
                    if entry.len() >= MAX_PEERS_PER_BUCKET {
                        continue;
                    } else {
                        entry.push_back(candidate);
                    }
                }
                Entry::Vacant(entry) => {
                    let _ = entry.insert(VecDeque::from([candidate]));
                }
            }
        }

        info!(
            "Time to generate NetworkDiscoveryCandidates: {:?}",
            start.elapsed()
        );
        let mut buckets_covered = candidates
            .iter()
            .map(|(ilog2, candidates)| (*ilog2, candidates.len()))
            .collect::<Vec<_>>();
        buckets_covered.sort_by_key(|(ilog2, _)| *ilog2);
        info!("The generated network discovery candidates currently cover these ilog2 buckets: {buckets_covered:?}");

        Self {
            self_key,
            candidates,
        }
    }

    /// Tries to refresh our current candidate list. The candidates at the front of the list are used when querying the
    /// network, so if a new peer for that bucket is generated, the first candidate is removed and the new candidate
    /// is inserted at the last
    pub(crate) fn try_refresh_candidates(&mut self) {
        let candidates_vec = Self::generate_candidates(&self.self_key, GENERATION_ATTEMPTS);
        for (ilog2, candidate) in candidates_vec {
            match self.candidates.entry(ilog2) {
                Entry::Occupied(mut entry) => {
                    let entry = entry.get_mut();
                    if entry.len() >= MAX_PEERS_PER_BUCKET {
                        // pop the front (as it might have been already used for querying and insert the new one at the back
                        let _ = entry.pop_front();
                        entry.push_back(candidate);
                    } else {
                        entry.push_back(candidate);
                    }
                }
                Entry::Vacant(entry) => {
                    let _ = entry.insert(VecDeque::from([candidate]));
                }
            }
        }
    }

    /// Returns one candidate per bucket
    /// Todo: Limit the candidates to return. Favor the closest buckets.
    pub(crate) fn candidates(&self) -> impl Iterator<Item = &NetworkAddress> {
        self.candidates
            .values()
            .filter_map(|candidates| candidates.front())
    }

    /// The result from the kad::GetClosestPeers are again used to update our kbuckets if they're not full.
    pub(crate) fn handle_get_closest_query(&mut self, closest_peers: HashSet<PeerId>) {
        let now = Instant::now();
        for peer in closest_peers {
            let peer = NetworkAddress::from_peer(peer);
            let peer_key = peer.as_kbucket_key();
            if let Some(ilog2_distance) = peer_key.distance(&self.self_key).ilog2() {
                match self.candidates.entry(ilog2_distance) {
                    Entry::Occupied(mut entry) => {
                        let entry = entry.get_mut();
                        // extra check to make sure we don't insert the same peer again
                        if entry.len() >= MAX_PEERS_PER_BUCKET && !entry.contains(&peer) {
                            // pop the front (as it might have been already used for querying and insert the new one at the back
                            let _ = entry.pop_front();
                            entry.push_back(peer);
                        } else {
                            entry.push_back(peer);
                        }
                    }
                    Entry::Vacant(entry) => {
                        let _ = entry.insert(VecDeque::from([peer]));
                    }
                }
            }
        }
        trace!(
            "It took {:?} to NetworkDiscovery::handle get closest query",
            now.elapsed()
        );
    }

    /// Uses rayon to parallelize the generation
    fn generate_candidates(
        self_key: &KBucketKey<PeerId>,
        num_to_generate: usize,
    ) -> Vec<(u32, NetworkAddress)> {
        (0..num_to_generate)
            .into_par_iter()
            .filter_map(|_| {
                let candidate = NetworkAddress::from_peer(PeerId::random());
                let candidate_key = candidate.as_kbucket_key();
                let ilog2_distance = candidate_key.distance(&self_key).ilog2()?;
                Some((ilog2_distance, candidate))
            })
            .collect::<Vec<_>>()
    }
}
