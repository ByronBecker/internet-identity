use hashtree::{Hash, HashTree};
use ic_cdk::api::{data_certificate, set_certified_data, time, trap};
use ic_cdk::export::candid::{CandidType, Deserialize, Principal};
use ic_cdk::storage::{stable_restore, stable_save};
use ic_cdk_macros::{init, post_upgrade, pre_upgrade, query, update};
use idp_service::signature_map::SignatureMap;
use serde::Serialize;
use std::cell::RefCell;
use std::collections::HashMap;

const DEFAULT_EXPIRATION_PERIOD_NS: u64 = 31_536_000_000_000_000;
const DEFAULT_SIGNATURE_EXPIRATION_PERIOD_NS: u64 = 600_000_000_000;

type UserId = u64;
type CredentialId = Vec<u8>;
type PublicKey = Vec<u8>;
type Alias = String;
type Entry = (Alias, PublicKey, Timestamp, Option<CredentialId>);
type Timestamp = u64;
type Signature = Vec<u8>;

#[derive(Clone, Debug, CandidType, Deserialize)]
struct Delegation {
    pubkey: PublicKey,
    expiration: Timestamp,
    targets: Option<Vec<Principal>>,
}

#[derive(Clone, Debug, CandidType, Deserialize)]
struct SignedDelegation {
    delegation: Delegation,
    signature: Signature,
}

mod hash;

#[derive(Clone, Debug, CandidType, Deserialize)]
struct HeaderField {
    key: String,
    value: String,
}

#[derive(Clone, Debug, CandidType, Deserialize)]
struct HttpRequest {
    method: String,
    url: String,
    headers: Vec<HeaderField>,
    body: Vec<u8>,
}

#[derive(Clone, Debug, CandidType, Deserialize)]
struct HttpResponse {
    status_code: u16,
    headers: Vec<HeaderField>,
    body: Vec<u8>,
}

struct State {
    map: RefCell<HashMap<UserId, Vec<Entry>>>,
    sigs: RefCell<SignatureMap>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            map: RefCell::new(HashMap::default()),
            sigs: RefCell::new(SignatureMap::default()),
        }
    }
}

thread_local! {
    static STATE: State = State::default();
    static ASSETS: RefCell<HashMap<String, Vec<u8>>> = RefCell::new(HashMap::default());
}

#[update]
fn register(user_id: UserId, alias: Alias, pk: PublicKey, credential_id: Option<CredentialId>) {
    STATE.with(|s| {
        let mut m = s.map.borrow_mut();
        if m.get(&user_id).is_some() {
            trap("This user is already registered");
        }

        prune_expired_signatures(&mut s.sigs.borrow_mut());

        let expiration = time() as u64 + DEFAULT_EXPIRATION_PERIOD_NS;
        m.insert(
            user_id,
            vec![(alias, pk.clone(), expiration, credential_id)],
        );
        add_signature(&mut s.sigs.borrow_mut(), user_id, pk, expiration);
    })
}

#[update]
fn add(user_id: UserId, alias: Alias, pk: PublicKey, credential: Option<CredentialId>) {
    STATE.with(|s| {
        let mut m = s.map.borrow_mut();
        if let Some(entries) = m.get_mut(&user_id) {
            let expiration = time() as u64 + DEFAULT_EXPIRATION_PERIOD_NS;
            for e in entries.iter_mut() {
                if e.1 == pk {
                    e.0 = alias;
                    e.2 = expiration;
                    e.3 = credential;
                    add_signature(&mut s.sigs.borrow_mut(), user_id, pk, expiration);
                    prune_expired_signatures(&mut s.sigs.borrow_mut());
                    return;
                }
            }
            entries.push((alias, pk.clone(), expiration, credential));
            add_signature(&mut s.sigs.borrow_mut(), user_id, pk, expiration);
            prune_expired_signatures(&mut s.sigs.borrow_mut());
        } else {
            trap("This user is not registered yet");
        }
    })
}

#[update]
fn remove(user_id: UserId, pk: PublicKey) {
    STATE.with(|s| {
        prune_expired_signatures(&mut s.sigs.borrow_mut());

        let mut remove_user = false;
        if let Some(entries) = s.map.borrow_mut().get_mut(&user_id) {
            if let Some(i) = entries.iter().position(|e| e.1 == pk) {
                let (_, _, expiration, _) = entries.swap_remove(i as usize);
                remove_signature(&mut s.sigs.borrow_mut(), user_id, pk, expiration);
                remove_user = entries.is_empty();
            }
        }
        if remove_user {
            s.map.borrow_mut().remove(&user_id);
        }
    })
}

#[query]
fn lookup(user_id: UserId) -> Vec<Entry> {
    STATE.with(|s| s.map.borrow().get(&user_id).cloned().unwrap_or_default())
}

#[query]
fn http_request(req: HttpRequest) -> HttpResponse {
    let parts: Vec<&str> = req.url.split("?").collect();
    let asset = parts[0].to_string();

    ASSETS.with(|a| match a.borrow().get(&asset) {
        Some(value) => HttpResponse {
            status_code: 200,
            headers: vec![],
            body: value.clone(),
        },
        None => HttpResponse {
            status_code: 404,
            headers: vec![],
            body: format!("Asset {} not found.", asset).as_bytes().into(),
        },
    })
}

#[query]
fn get_delegation(user_id: UserId, pubkey: PublicKey) -> SignedDelegation {
    STATE.with(|state| {
        let mut m = state.map.borrow_mut();
        if let Some(entries) = m.get_mut(&user_id) {
            if let Some((_, _, expiration, _)) = entries.iter().find(|e| e.1 == pubkey) {
                let signature =
                    get_signature(&state.sigs.borrow(), user_id, pubkey.clone(), *expiration)
                        .unwrap_or_else(|| trap("No signature found"));
                return SignedDelegation {
                    delegation: Delegation {
                        pubkey,
                        expiration: *expiration,
                        targets: None,
                    },
                    signature,
                };
            }
        }
        trap("User ID and public key pair not found.");
    })
}

// used both in init and post_upgrade
fn init_assets() {
    ASSETS.with(|a| {
        let mut a = a.borrow_mut();

        a.insert(
            "/sample-asset.txt".to_string(),
            include_str!("../../frontend/assets/sample-asset.txt")
                .as_bytes()
                .into(),
        );
    });
}

#[init]
fn init() {
    STATE.with(|state| update_root_hash(&state.sigs.borrow()));
    init_assets();
}

#[pre_upgrade]
fn persist_data() {
    STATE.with(|s| {
        let map = s.map.replace(Default::default());
        if let Err(err) = stable_save((map,)) {
            ic_cdk::trap(&format!(
                "An error occurred while saving data to stable memory: {}",
                err
            ));
        }
    })
}

#[post_upgrade]
fn retrieve_data() {
    init_assets();
    match stable_restore::<(HashMap<UserId, Vec<Entry>>,)>() {
        Ok((map,)) => {
            STATE.with(|s| {
                // Restore user map.
                s.map.replace(map);

                // We drop all the signatures on upgrade, users will
                // re-request them if needed.
                update_root_hash(&s.sigs.borrow());
            });
        }
        Err(err) => ic_cdk::trap(&format!(
            "An error occurred while retrieving data from stable memory: {}",
            err
        )),
    }
}

fn hash_seed(user_id: UserId) -> Hash {
    hash::hash_string(user_id.to_string().as_str())
}

fn delegation_signature_msg_hash(d: &Delegation) -> Hash {
    use hash::Value;

    let mut m = HashMap::new();
    m.insert("pubkey", Value::Bytes(d.pubkey.as_slice()));
    m.insert("expiration", Value::U64(d.expiration));
    if let Some(targets) = d.targets.as_ref() {
        let mut arr = Vec::with_capacity(targets.len());
        for t in targets.iter() {
            arr.push(Value::Bytes(t.as_ref()));
        }
        m.insert("targets", Value::Array(arr));
    }
    let map_hash = hash::hash_of_map(m);
    hash::hash_with_domain(b"ic-request-auth-delegation", &map_hash)
}

fn update_root_hash(m: &SignatureMap) {
    let prefixed_root_hash = hashtree::labeled_hash(b"sig", &m.root_hash());
    set_certified_data(&prefixed_root_hash[..]);
}

fn get_signature(
    sigs: &SignatureMap,
    user_id: UserId,
    pk: PublicKey,
    expiration: Timestamp,
) -> Option<Vec<u8>> {
    let certificate = data_certificate()?;
    let msg_hash = delegation_signature_msg_hash(&Delegation {
        pubkey: pk,
        expiration,
        targets: None,
    });
    let witness = sigs.witness(hash_seed(user_id), msg_hash)?;
    let tree = HashTree::Labeled(&b"sig"[..], Box::new(witness));

    #[derive(Serialize)]
    struct Sig<'a> {
        #[serde(with = "serde_bytes")]
        certificate: Vec<u8>,
        tree: HashTree<'a>,
    }

    let sig = Sig { certificate, tree };

    let mut cbor = serde_cbor::ser::Serializer::new(Vec::new());
    cbor.self_describe().unwrap();
    sig.serialize(&mut cbor).unwrap();
    Some(cbor.into_inner())
}

fn add_signature(sigs: &mut SignatureMap, user_id: UserId, pk: PublicKey, expiration: Timestamp) {
    let msg_hash = delegation_signature_msg_hash(&Delegation {
        pubkey: pk,
        expiration,
        targets: None,
    });
    let expires_at = time() as u64 + DEFAULT_SIGNATURE_EXPIRATION_PERIOD_NS;
    sigs.put(hash_seed(user_id), msg_hash, expires_at);
    update_root_hash(&sigs);
}

fn remove_signature(
    sigs: &mut SignatureMap,
    user_id: UserId,
    pk: PublicKey,
    expiration: Timestamp,
) {
    let msg_hash = delegation_signature_msg_hash(&Delegation {
        pubkey: pk,
        expiration,
        targets: None,
    });
    sigs.delete(hash_seed(user_id), msg_hash);
    update_root_hash(sigs);
}

/// Removes a batch of expired signatures from the signature map.
///
/// This function is supposed to piggy back on update calls to
/// amortize the cost of tree pruning.  Each operation on the signature map
/// will prune at most MAX_SIGS_TO_PRUNE other signatures.
fn prune_expired_signatures(sigs: &mut SignatureMap) {
    const MAX_SIGS_TO_PRUNE: usize = 10;
    let num_pruned = sigs.prune_expired(time() as u64, MAX_SIGS_TO_PRUNE);

    if num_pruned > 0 {
        update_root_hash(sigs);
    }
}

fn main() {}
