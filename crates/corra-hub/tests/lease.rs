use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Result, bail};
use corra_hub::storage::{Cond, Lease, LeaseStore, Lost, Put, Timing, keep, release, take};

#[derive(Default)]
struct Bucket {
    object: Mutex<Option<(Lease, String)>>,
    down: AtomicBool,
}

fn content_tag(lease: &Lease) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    serde_json::to_string(lease).unwrap().hash(&mut h);
    format!("\"{:x}\"", h.finish())
}

#[async_trait::async_trait]
impl LeaseStore for Bucket {
    async fn get(&self) -> Result<Option<(Lease, String)>> {
        if self.down.load(Ordering::SeqCst) {
            bail!("unreachable");
        }
        Ok(self.object.lock().unwrap().clone())
    }

    async fn put(&self, lease: &Lease, cond: Cond) -> Result<Put> {
        if self.down.load(Ordering::SeqCst) {
            bail!("unreachable");
        }
        let mut obj = self.object.lock().unwrap();
        let ok = match (&cond, &*obj) {
            (Cond::Absent, None) => true,
            (Cond::Match(want), Some((_, tag))) => want == tag,
            _ => false,
        };
        if !ok {
            return Ok(Put::Conflict);
        }
        *obj = Some((lease.clone(), content_tag(lease)));
        Ok(Put::Done)
    }
}

const FAST: Timing = Timing {
    renew: Duration::from_millis(20),
    give_up: Duration::from_millis(150),
    stale_ms: 300,
    poll: Duration::from_millis(10),
};

fn holder(b: &Bucket) -> Option<String> {
    b.object
        .lock()
        .unwrap()
        .as_ref()
        .map(|(l, _)| l.holder.clone())
}

#[tokio::test]
async fn the_second_hub_waits_while_the_first_renews_and_takes_over_at_handover() {
    let b = Arc::new(Bucket::default());
    take(b.as_ref(), "one", FAST).await.unwrap();
    let released = Arc::new(AtomicBool::new(false));
    let first = {
        let (b, r) = (b.clone(), released.clone());
        tokio::spawn(async move { keep(b.as_ref(), "one", FAST, &r).await })
    };
    let second = {
        let b = b.clone();
        tokio::spawn(async move { take(b.as_ref(), "two", FAST).await })
    };
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert!(!second.is_finished(), "took the lease from a live hub");
    assert_eq!(holder(&b).as_deref(), Some("one"));

    released.store(true, Ordering::SeqCst);
    assert_eq!(first.await.unwrap(), None);
    release(b.as_ref(), "one").await.unwrap();
    tokio::time::timeout(Duration::from_millis(200), second)
        .await
        .expect("a released lease wasn't taken at once")
        .unwrap()
        .unwrap();
    assert_eq!(holder(&b).as_deref(), Some("two"));
}

#[tokio::test]
async fn a_stale_lease_is_taken_and_its_old_holder_stops() {
    let b = Arc::new(Bucket::default());
    take(b.as_ref(), "one", FAST).await.unwrap();
    tokio::time::sleep(Duration::from_millis(350)).await;
    take(b.as_ref(), "two", FAST).await.unwrap();
    assert_eq!(holder(&b).as_deref(), Some("two"));
    let never = AtomicBool::new(false);
    assert_eq!(
        keep(b.as_ref(), "one", FAST, &never).await,
        Some(Lost::TakenBy("two".into()))
    );
}

#[tokio::test]
async fn a_hub_that_cant_renew_stops_before_its_lease_goes_stale() {
    let b = Arc::new(Bucket::default());
    take(b.as_ref(), "one", FAST).await.unwrap();
    b.down.store(true, Ordering::SeqCst);
    let started = std::time::Instant::now();
    let never = AtomicBool::new(false);
    assert_eq!(
        keep(b.as_ref(), "one", FAST, &never).await,
        Some(Lost::Unreachable)
    );
    assert!(started.elapsed() < Duration::from_millis(FAST.stale_ms as u64));
}

#[tokio::test]
async fn two_hubs_starting_at_once_get_one_lease() {
    for _ in 0..50 {
        let b = Arc::new(Bucket::default());
        let racers: Vec<_> = ["one", "two", "three"]
            .into_iter()
            .map(|h| {
                let b = b.clone();
                tokio::spawn(async move {
                    tokio::time::timeout(Duration::from_millis(100), take(b.as_ref(), h, FAST))
                        .await
                        .is_ok()
                })
            })
            .collect();
        let mut won = 0;
        for r in racers {
            won += r.await.unwrap() as usize;
        }
        assert_eq!(won, 1);
    }
}

#[tokio::test]
async fn a_hub_cant_release_a_lease_it_lost() {
    let b = Bucket::default();
    take(&b, "one", FAST).await.unwrap();
    tokio::time::sleep(Duration::from_millis(350)).await;
    take(&b, "two", FAST).await.unwrap();
    assert!(release(&b, "one").await.is_err());
    assert_eq!(holder(&b).as_deref(), Some("two"));
}

#[tokio::test]
#[ignore = "needs a real bucket in CORRA_STORAGE_*"]
async fn a_real_bucket_keeps_the_same_rules() {
    let st = corra_hub::storage::from_env()
        .unwrap()
        .expect("CORRA_STORAGE_* set");
    let st: &dyn LeaseStore = st.as_ref();
    let slow = Timing {
        renew: Duration::from_millis(200),
        give_up: Duration::from_secs(3),
        stale_ms: 4_000,
        poll: Duration::from_millis(200),
    };
    let me = format!("one-{}", corra_proto::new_id());
    let other = format!("two-{}", corra_proto::new_id());
    if let Some((old, tag)) = st.get().await.unwrap() {
        let freed = Lease {
            released: true,
            ..old
        };
        assert_eq!(st.put(&freed, Cond::Match(tag)).await.unwrap(), Put::Done);
    }
    take(st, &me, slow).await.unwrap();
    let (lease, tag) = st.get().await.unwrap().unwrap();
    assert_eq!(lease.holder, me);

    assert_eq!(st.put(&lease, Cond::Absent).await.unwrap(), Put::Conflict);
    assert_eq!(
        st.put(&lease, Cond::Match("\"0000\"".into()))
            .await
            .unwrap(),
        Put::Conflict
    );
    let renewed = Lease {
        at: lease.at + 1,
        ..lease.clone()
    };
    assert_eq!(
        st.put(&renewed, Cond::Match(tag.clone())).await.unwrap(),
        Put::Done
    );
    assert_eq!(
        st.put(&lease, Cond::Match(tag)).await.unwrap(),
        Put::Conflict
    );

    let waited = tokio::time::timeout(Duration::from_secs(2), take(st, &other, slow)).await;
    assert!(waited.is_err(), "took a fresh lease from another holder");

    release(st, &me).await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), take(st, &other, slow))
        .await
        .expect("a released lease wasn't taken")
        .unwrap();
    let never = AtomicBool::new(false);
    assert!(matches!(
        keep(st, &me, slow, &never).await,
        Some(Lost::TakenBy(h)) if h == other
    ));
    release(st, &other).await.unwrap();
}
