//! The journal's rows: states as text and back, which items are still to
//! decide, and forgetting old ones.
use super::journal::{self, ItemState, Kind, MemberState, NewMember};
use super::preconditions::Preconditions;
use super::tests::fixture;
use crate::fsops;
use crate::storage::Identity;
use anyhow::Result;

#[test]
fn every_state_reads_back_from_its_text() {
    for k in Kind::ALL {
        assert_eq!(k.as_str().parse::<Kind>().unwrap(), *k);
    }
    for s in ItemState::ALL {
        assert_eq!(s.as_str().parse::<ItemState>().unwrap(), *s);
    }
    for s in MemberState::ALL {
        assert_eq!(s.as_str().parse::<MemberState>().unwrap(), *s);
    }
    assert!("moving".parse::<ItemState>().is_err());
}

#[test]
fn items_round_trip_and_only_unfinished_ones_are_unresolved() -> Result<()> {
    let mut fx = fixture(&["a.NEF"]);
    let a = fx.photo("a.NEF");
    let path = fx.root.join("a.NEF");
    let member = NewMember {
        source: path.clone(),
        destination: Some(fx.root.join("b.NEF")),
        id: fsops::entry(&path)?.id,
        identity: Identity::read(&path)?,
    };
    let pre = Preconditions::read(&fx.cat.db, a, vec![a])?.unwrap();
    let computer = fx.cat.computer.id.clone();
    let mut ids = Vec::new();
    fx.cat.db.write(|w| {
        for state in ItemState::ALL {
            ids.push(journal::insert(
                w,
                "batch",
                &computer,
                Kind::MovePhotos,
                *state,
                &pre,
                std::slice::from_ref(&member),
                Some("why"),
            )?);
        }
        Ok(())
    })?;
    let all = journal::of_batch(&fx.cat.db, "batch")?;
    assert_eq!(all.len(), ItemState::ALL.len());
    let first = &all[0];
    assert_eq!(first.subject, journal::Subject::Photos(pre.clone()));
    assert_eq!(first.members[0].source, member.source);
    assert_eq!(first.members[0].destination, member.destination);
    assert_eq!(first.members[0].id, Some(member.id));
    assert_eq!(first.members[0].identity.as_ref(), Some(&member.identity));
    assert_eq!(first.members[0].state, MemberState::Planned);
    assert_eq!(first.error.as_deref(), Some("why"));
    let unresolved: Vec<ItemState> = journal::unresolved(&fx.cat.db, &computer)?
        .iter()
        .map(|i| i.state)
        .collect();
    assert_eq!(
        unresolved,
        [
            ItemState::Planned,
            ItemState::Restoring,
            ItemState::Committed,
            ItemState::NeedsRecovery
        ]
    );
    // Another computer's are not this one's to decide.
    assert!(journal::unresolved(&fx.cat.db, "elsewhere")?.is_empty());
    // A member's step, with its receipt.
    let receipt = fx.root.join("Trash/a.NEF");
    fx.cat
        .db
        .write(|w| journal::set_member(w, ids[0], 0, MemberState::Trashed, Some(&receipt)))?;
    let item = journal::item(&fx.cat.db, ids[0])?.unwrap();
    assert_eq!(item.members[0].state, MemberState::Trashed);
    assert_eq!(item.members[0].receipt.as_deref(), Some(receipt.as_path()));
    // Finished ones are forgotten once old enough; the rest stay.
    let forgotten = fx
        .cat
        .db
        .write(|w| journal::prune(w, "9999-01-01 00:00:00"))?;
    assert_eq!(forgotten, 3);
    assert_eq!(journal::of_batch(&fx.cat.db, "batch")?.len(), 4);
    assert_eq!(
        fx.cat
            .db
            .write(|w| journal::prune(w, "2000-01-01 00:00:00"))?,
        0
    );
    Ok(())
}

#[test]
fn a_copy_round_trips_and_goes_with_its_item() -> Result<()> {
    use crate::catalog::db::{Reads, sql};
    let mut fx = fixture(&["a.NEF", "far/"]);
    let a = fx.photo("a.NEF");
    let path = fx.root.join("a.NEF");
    let member = NewMember {
        source: path.clone(),
        destination: Some(fx.root.join("far/a.NEF")),
        id: fsops::entry(&path)?.id,
        identity: Identity::read(&path)?,
    };
    let temp = fx.root.join("far/.a.NEF.x-0-0.rawmakase-part");
    let pre = Preconditions::read(&fx.cat.db, a, vec![a])?.unwrap();
    let computer = fx.cat.computer.id.clone();
    let id = fx.cat.db.write(|w| {
        let id = journal::insert(
            w,
            "batch",
            &computer,
            Kind::MovePhotos,
            ItemState::Planned,
            &pre,
            std::slice::from_ref(&member),
            None,
        )?;
        journal::insert_copies(w, id, std::slice::from_ref(&temp))?;
        Ok(id)
    })?;
    let item = journal::item(&fx.cat.db, id)?.unwrap();
    assert!(item.copies());
    let copy = item.members[0].copy.clone().unwrap();
    assert_eq!(copy.temp, temp);
    assert_eq!((copy.id, copy.content, copy.identity), (None, None, None));
    // Each step, as journalled.
    let file = fsops::entry(&fx.root)?.id;
    let content = fsops::hash_file(&path)?;
    let receipt = fx.root.join("Trash/a.NEF");
    fx.cat.db.write(|w| {
        journal::set_copying(w, id, 0, Some(file), &temp)?;
        journal::set_content(w, id, 0, content)?;
        journal::set_copy_identity(w, id, 0, &member.identity)?;
        journal::set_kept(w, id, 0, "its volume has no Trash")
    })?;
    let m = &journal::item(&fx.cat.db, id)?.unwrap().members[0];
    assert_eq!(m.state, MemberState::OriginalKept);
    let copy = m.copy.clone().unwrap();
    assert_eq!(copy.id, Some(file));
    assert_eq!(copy.content, Some(content));
    assert_eq!(copy.identity.as_ref(), Some(&member.identity));
    assert_eq!(copy.kept.as_deref(), Some("its volume has no Trash"));
    fx.cat
        .db
        .write(|w| journal::set_discarded(w, id, 0, Some(&receipt)))?;
    let m = &journal::item(&fx.cat.db, id)?.unwrap().members[0];
    assert_eq!(m.state, MemberState::Discarded);
    assert_eq!(
        m.copy.as_ref().unwrap().receipt.as_deref(),
        Some(receipt.as_path())
    );
    // Forgotten with its item.
    fx.cat
        .db
        .write(|w| journal::set_state(w, id, ItemState::Done, None))?;
    fx.cat
        .db
        .write(|w| journal::prune(w, "9999-01-01 00:00:00"))?;
    let left: i64 = fx
        .cat
        .db
        .read_one(sql!("SELECT count(*) FROM file_operation_copies"), &[])?;
    assert_eq!(left, 0);
    Ok(())
}
