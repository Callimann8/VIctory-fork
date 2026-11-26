//! Fewer group barriers around LDS in a console wave64 program run as two host waves of 32
//! (`VICTORY_LDS_PRUNE=1`, off by default).
//!
//! The console runs the 64 lanes of a wave in step, so its compiler leaves out barriers
//! between LDS accesses of one wave. Split into two host waves, the halves no longer run in
//! step, and the translation (`Lower::split_lds`) has them meet before *every* LDS access.
//! The order the console keeps only matters between two accesses of which one writes: two
//! reads with no write between them see the same bytes whichever half runs first. So a
//! meeting before an access is needed only when, on some way from the last meeting to it,
//!  * a write happened (any access after a write must wait for it), or
//!  * the access writes and any access happened (a write must wait for the reads and writes
//!    before it).
//!
//! That is followed over the program's blocks (a forward data flow over "some access / some
//! write since the last meeting", joined over every way in, loops until nothing changes).
//! Every meeting the translation adds elsewhere (the title's waiting `s_barrier`s, meetings
//! around lane branches, block-end flushes below) clears the state; meetings this pass cannot
//! see are simply not counted (more meetings, never fewer).
//!
//! A loop that reads LDS after a write before it would still meet once per iteration (the
//! first read of the body sees the write through the way into the loop). Where the block
//! before such a loop is one every lane runs, the halves meet once at its end instead
//! ("flush"), and the reads inside need no meeting while the loop writes nothing.
//!
//! The result is exact in the console's terms: every pair of accesses on one way with a
//! write among them still has a meeting between them. Only meetings between reads and
//! before a first access go.

use std::collections::HashSet;

/// One item of a block, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Item {
    /// An LDS access at instruction `at`; `meet`: the translation may put a meeting right
    /// before it (otherwise it runs where the halves are not sure to meet: a lane branch's
    /// region, a whole-wave section).
    Lds { at: usize, write: bool, meet: bool },
    /// A meeting of the whole group that is there anyway (a waiting `s_barrier`).
    Sync,
}

/// One block of the program, as this pass sees it.
#[derive(Debug, Clone, Default)]
pub(crate) struct Block {
    pub items: Vec<Item>,
    pub succ: Vec<usize>,
    /// The halves meet at its start / before its exit anyway (`lds_meetings`).
    pub start_meet: bool,
    pub end_meet: bool,
    /// Every lane of the group runs it: a meeting may be added before its exit.
    pub can_flush: bool,
}

/// What to leave out and what to add.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Plan {
    /// (block, instruction) of LDS accesses that need no meeting before them.
    pub skip: HashSet<(usize, usize)>,
    /// Blocks before whose exit the halves meet (added).
    pub flush: HashSet<usize>,
    /// Meetings before accesses kept (static count, for the log line).
    pub kept: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct State {
    any: bool,
    wrote: bool,
}

impl State {
    fn join(self, o: State) -> State {
        State { any: self.any || o.any, wrote: self.wrote || o.wrote }
    }
}

fn needs(s: State, write: bool) -> bool {
    s.wrote || (write && s.any)
}

/// Reverse postorder from block 0.
fn rpo(blocks: &[Block]) -> Vec<usize> {
    let n = blocks.len();
    let mut seen = vec![false; n];
    let mut post = Vec::with_capacity(n);
    if n == 0 {
        return post;
    }
    let mut stack = vec![(0usize, 0usize)];
    seen[0] = true;
    while let Some(&mut (b, ref mut next)) = stack.last_mut() {
        if let Some(&t) = blocks[b].succ.get(*next) {
            *next += 1;
            if t < n && !seen[t] {
                seen[t] = true;
                stack.push((t, 0));
            }
        } else {
            post.push(b);
            stack.pop();
        }
    }
    post.reverse();
    post
}

/// The plan for `blocks` (block 0 is the entry).
pub(crate) fn plan(blocks: &[Block]) -> Plan {
    let n = blocks.len();
    let order = rpo(blocks);
    let mut place = vec![usize::MAX; n];
    for (i, &b) in order.iter().enumerate() {
        place[b] = i;
    }
    let mut preds: Vec<Vec<usize>> = vec![Vec::new(); n];
    for &b in &order {
        for &t in &blocks[b].succ {
            if t < n {
                preds[t].push(b);
            }
        }
    }
    let mut out = Plan::default();
    // Loop flushes: per back edge t -> h, the loop's body (h and what reaches t without
    // passing h); if the body has an access the translation would meet before, the halves
    // meet at the end of every way into h from outside, where every lane runs it.
    for &t in &order {
        for &h in &blocks[t].succ {
            if h >= n || place[h] > place[t] {
                continue;
            }
            let mut body = vec![false; n];
            body[h] = true;
            let mut work = vec![t];
            while let Some(b) = work.pop() {
                if body[b] {
                    continue;
                }
                body[b] = true;
                work.extend(preds[b].iter().copied());
            }
            let meets_inside = (0..n)
                .filter(|&b| body[b])
                .any(|b| blocks[b].items.iter().any(|i| matches!(i, Item::Lds { meet: true, .. })));
            if !meets_inside {
                continue;
            }
            for &p in &preds[h] {
                if !body[p] && blocks[p].can_flush {
                    out.flush.insert(p);
                }
            }
        }
    }
    // Forward flow to a fixed point (the transfer is monotone: after an access the state is
    // (some access, whether it wrote) whether or not a meeting came before it). `own` follows
    // only the accesses this pass places meetings before: an access without a place for one
    // (a whole-wave section) that a write of theirs reaches with no meeting between could
    // have been ordered by a meeting this pass would leave out, so then nothing is left out.
    let transfer = |b: usize, mut s: [State; 2], record: Option<&mut Plan>, unsafe_: &mut bool| -> [State; 2] {
        let block = &blocks[b];
        let mut record = record;
        if block.start_meet {
            s = [State::default(); 2];
        }
        for item in &block.items {
            match *item {
                Item::Sync => s = [State::default(); 2],
                Item::Lds { at, write, meet } => {
                    if meet {
                        if needs(s[0], write) {
                            s = [State::default(); 2];
                            if let Some(r) = record.as_deref_mut() {
                                r.kept += 1;
                            }
                        } else if let Some(r) = record.as_deref_mut() {
                            r.skip.insert((b, at));
                        }
                        s[1].any = true;
                        s[1].wrote |= write;
                    } else if needs(s[1], write) {
                        *unsafe_ = true;
                    }
                    s[0].any = true;
                    s[0].wrote |= write;
                }
            }
        }
        if block.end_meet || out.flush.contains(&b) {
            s = [State::default(); 2];
        }
        s
    };
    let join = |a: [State; 2], o: [State; 2]| [a[0].join(o[0]), a[1].join(o[1])];
    let mut inp = vec![[State::default(); 2]; n];
    let mut outp = vec![[State::default(); 2]; n];
    let mut unsafe_ = false;
    loop {
        let mut changed = false;
        for &b in &order {
            let s = preds[b].iter().fold([State::default(); 2], |a, &p| join(a, outp[p]));
            inp[b] = s;
            let o = transfer(b, s, None, &mut unsafe_);
            if o != outp[b] {
                outp[b] = o;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    if unsafe_ {
        return Plan::default();
    }
    let mut rec = Plan { skip: HashSet::new(), flush: out.flush.clone(), kept: 0 };
    for &b in &order {
        transfer(b, inp[b], Some(&mut rec), &mut unsafe_);
    }
    rec
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(at: usize) -> Item {
        Item::Lds { at, write: false, meet: true }
    }
    fn w(at: usize) -> Item {
        Item::Lds { at, write: true, meet: true }
    }
    fn blk(items: Vec<Item>, succ: Vec<usize>) -> Block {
        Block { items, succ, can_flush: true, ..Default::default() }
    }

    /// Every way through the blocks (each block at most three times on it): every pair of
    /// accesses with a write among them that the translation without this pass orders (a
    /// meeting between them) the plan orders too. Brute force over short paths.
    fn check(blocks: &[Block], p: &Plan) {
        let none = Plan::default();
        #[derive(Clone, Copy)]
        enum Ev {
            Acc(bool),
            Meet,
        }
        let block_events = |b: usize, p: &Plan| -> Vec<Ev> {
            let mut v = Vec::new();
            let blk = &blocks[b];
            if blk.start_meet {
                v.push(Ev::Meet);
            }
            for it in &blk.items {
                match *it {
                    Item::Sync => v.push(Ev::Meet),
                    Item::Lds { at, write, meet } => {
                        if meet && !p.skip.contains(&(b, at)) {
                            v.push(Ev::Meet);
                        }
                        v.push(Ev::Acc(write));
                    }
                }
            }
            if blk.end_meet || p.flush.contains(&b) {
                v.push(Ev::Meet);
            }
            v
        };
        // per path: for each access, its write flag and the number of accesses before the
        // last meeting before it, under each plan
        fn fence(trail: &[Ev]) -> Vec<(bool, usize)> {
            let (mut out, mut seen, mut last) = (Vec::new(), 0usize, 0usize);
            for e in trail {
                match *e {
                    Ev::Meet => last = seen,
                    Ev::Acc(w) => {
                        out.push((w, last));
                        seen += 1;
                    }
                }
            }
            out
        }
        fn walk(
            b: usize,
            blocks: &[Block],
            visits: &mut Vec<u8>,
            trails: &mut (Vec<Ev>, Vec<Ev>),
            ev: &dyn Fn(usize, bool) -> Vec<Ev>,
        ) {
            if visits[b] >= 3 {
                return;
            }
            visits[b] += 1;
            let lens = (trails.0.len(), trails.1.len());
            trails.0.extend(ev(b, false));
            trails.1.extend(ev(b, true));
            let (a, c) = (fence(&trails.0), fence(&trails.1));
            for k in 0..a.len() {
                for j in 0..k {
                    let conflict = a[j].0 || a[k].0;
                    let ordered_before = a[k].1 > j;
                    let ordered_now = c[k].1 > j;
                    assert!(
                        !(conflict && ordered_before && !ordered_now),
                        "accesses {j} and {k}: ordered without the plan, not with it"
                    );
                }
            }
            for &t in &blocks[b].succ {
                walk(t, blocks, visits, trails, ev);
            }
            trails.0.truncate(lens.0);
            trails.1.truncate(lens.1);
            visits[b] -= 1;
        }
        let ev = |b: usize, with: bool| block_events(b, if with { p } else { &none });
        let mut visits = vec![0u8; blocks.len()];
        let mut trails = (Vec::new(), Vec::new());
        walk(0, blocks, &mut visits, &mut trails, &ev);
    }

    #[test]
    fn reads_after_a_meeting_share_it() {
        let blocks = vec![blk(vec![w(0), r(1), r(2), r(3)], vec![])];
        let p = plan(&blocks);
        check(&blocks, &p);
        // the first write needs none (nothing before it), the first read one, the others none
        assert_eq!(p.skip, HashSet::from([(0, 0), (0, 2), (0, 3)]));
        assert_eq!(p.kept, 1);
    }

    #[test]
    fn writes_keep_their_meetings() {
        let blocks = vec![blk(vec![r(0), w(1), w(2), r(3)], vec![])];
        let p = plan(&blocks);
        check(&blocks, &p);
        assert_eq!(p.skip, HashSet::from([(0, 0)]));
        assert_eq!(p.kept, 3);
    }

    #[test]
    fn a_read_loop_after_a_write_meets_once_before_it() {
        // 0: write; 1: loop head reads; 2: body reads, back to 1; 3: exit
        let blocks = vec![
            blk(vec![w(0)], vec![1]),
            blk(vec![r(0)], vec![2, 3]),
            blk(vec![r(0), r(1)], vec![1]),
            blk(vec![], vec![]),
        ];
        let p = plan(&blocks);
        check(&blocks, &p);
        assert!(p.flush.contains(&0));
        assert!(p.skip.contains(&(1, 0)) && p.skip.contains(&(2, 0)) && p.skip.contains(&(2, 1)));
    }

    #[test]
    fn a_loop_that_writes_keeps_its_meetings() {
        let blocks = vec![
            blk(vec![w(0)], vec![1]),
            blk(vec![r(0), w(1)], vec![1, 2]),
            blk(vec![r(0)], vec![]),
        ];
        let p = plan(&blocks);
        check(&blocks, &p);
        assert!(!p.skip.contains(&(1, 0)) && !p.skip.contains(&(1, 1)) && !p.skip.contains(&(2, 0)));
    }

    #[test]
    fn no_flush_where_not_every_lane_runs() {
        let mut blocks = vec![
            blk(vec![w(0)], vec![1]),
            blk(vec![r(0)], vec![1, 2]),
            blk(vec![], vec![]),
        ];
        blocks[0].can_flush = false;
        let p = plan(&blocks);
        check(&blocks, &p);
        assert!(p.flush.is_empty());
        assert!(!p.skip.contains(&(1, 0)));
    }

    #[test]
    fn region_accesses_count_but_get_no_meeting() {
        // 0 branches into a lane region 1 (a write without meeting) and joins at 2, which
        // starts with a meeting (lds_meetings); 2 reads.
        let mut blocks = vec![
            blk(vec![r(0)], vec![1, 2]),
            blk(vec![Item::Lds { at: 0, write: true, meet: false }], vec![2]),
            blk(vec![r(0), r(1)], vec![]),
        ];
        blocks[0].end_meet = true;
        blocks[2].start_meet = true;
        let p = plan(&blocks);
        check(&blocks, &p);
        assert!(p.skip.contains(&(2, 0)) && p.skip.contains(&(2, 1)));
        // without the meetings around the region its write would follow block 0's read with
        // nothing between: such a program keeps every meeting
        blocks[2].start_meet = false;
        blocks[0].end_meet = false;
        let p = plan(&blocks);
        check(&blocks, &p);
        assert_eq!(p, Plan::default());
    }

    #[test]
    fn title_barriers_clear_the_state() {
        let blocks = vec![blk(vec![w(0), Item::Sync, r(2), w(3)], vec![])];
        let p = plan(&blocks);
        check(&blocks, &p);
        assert_eq!(p.skip, HashSet::from([(0, 0), (0, 2)]));
    }

    /// Random small programs: the plan always keeps the console's order.
    #[test]
    fn random_programs_keep_the_order() {
        let mut seed = 0x2545_F491_4F6C_DD1Du64;
        let mut next = |m: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % m
        };
        for _ in 0..3000 {
            let n = 1 + next(6) as usize;
            let mut blocks = Vec::new();
            for b in 0..n {
                let mut items = Vec::new();
                for at in 0..next(4) as usize {
                    items.push(match next(7) {
                        0 => Item::Sync,
                        1 => Item::Lds { at, write: next(2) == 0, meet: false },
                        k => Item::Lds { at, write: k % 2 == 0, meet: true },
                    });
                }
                let mut succ = Vec::new();
                for _ in 0..next(3) {
                    succ.push(next(n as u64) as usize);
                }
                let _ = b;
                blocks.push(Block {
                    items,
                    succ,
                    start_meet: next(5) == 0,
                    end_meet: next(5) == 0,
                    can_flush: next(2) == 0,
                });
            }
            let p = plan(&blocks);
            check(&blocks, &p);
        }
    }
}
