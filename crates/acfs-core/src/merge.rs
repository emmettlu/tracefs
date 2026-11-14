use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::path::{self, RelPath};
use crate::rules::{Rules, Strategy};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ChangeKind {
    Add,
    Modify,
    Delete,
    Rename,
}

pub type Kinds = BTreeMap<RelPath, ChangeKind>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FType {
    Reg,
    Dir,
    Symlink,
    Fifo,
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Meta {
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry {
    pub ftype: FType,
    pub meta: Meta,
    pub size: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Side {
    Base,
    Source,
    Target,
}

pub trait Inspect {
    fn entry(&mut self, side: Side, p: &[u8]) -> Option<Entry>;
    fn content(&mut self, side: Side, p: &[u8]) -> Vec<u8>;
    fn is_text(&mut self, side: Side, p: &[u8]) -> bool;
    fn xattrs(&mut self, side: Side, p: &[u8]) -> Vec<(Vec<u8>, Vec<u8>)>;
    fn ident(&mut self, side: Side, p: &[u8]) -> Option<u64>;
    fn mtime(&mut self, side: Side, p: &[u8]) -> (i64, i64);
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Write,
    WriteContent,
    Meta,
    TextMerge { meta_from: Side, union: bool },
    Content(Vec<u8>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    pub action: Action,
    pub o: Option<RelPath>,
    pub s: Option<RelPath>,
    pub t: Option<RelPath>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Move {
    pub from: RelPath,
    pub to: RelPath,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Resolution {
    Ours,
    Theirs,
    Content(Vec<u8>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conflict {
    pub path: RelPath,
    pub reason: &'static str,
}

#[derive(Debug, Default)]
pub struct Plan {
    pub deletes: BTreeSet<RelPath>,
    pub moves: Vec<Move>,
    pub writes: BTreeMap<RelPath, Step>,
    pub conflicts: Vec<Conflict>,
    pub unused_resolutions: Vec<RelPath>,
}

impl Plan {
    pub fn len(&self) -> usize {
        self.deletes.len() + self.moves.len() + self.writes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

pub struct Input<'a> {
    pub source: &'a Kinds,
    pub target: &'a Kinds,
    pub idents: (bool, bool),
    pub resolutions: &'a BTreeMap<RelPath, Resolution>,
    pub rules: &'a Rules,
}

#[derive(Default)]
struct Moves {
    to: BTreeMap<RelPath, RelPath>,
    from: BTreeMap<RelPath, RelPath>,
}

impl Moves {
    fn detect(kinds: &Kinds, side: Side, idents: bool, i: &mut dyn Inspect) -> Moves {
        fn note(m: &mut HashMap<u64, Option<RelPath>>, id: u64, p: &RelPath) {
            m.entry(id).and_modify(|e| *e = None).or_insert_with(|| Some(p.clone()));
        }
        let (mut left, mut arrived) = (HashMap::new(), HashMap::new());
        for (p, k) in kinds.iter().filter(|_| idents) {
            if *k != ChangeKind::Add
                && let Some(id) = i.ident(Side::Base, p)
            {
                note(&mut left, id, p);
            }
            if *k != ChangeKind::Delete
                && let Some(id) = i.ident(side, p)
            {
                note(&mut arrived, id, p);
            }
        }
        let mut m = Moves::default();
        for (id, sp) in arrived {
            let (Some(sp), Some(Some(op))) = (sp, left.get(&id)) else { continue };
            if sp == *op || i.entry(Side::Base, op).map(|e| e.ftype) != i.entry(side, &sp).map(|e| e.ftype) {
                continue;
            }
            m.to.insert(op.clone(), sp.clone());
            m.from.insert(sp, op.clone());
        }
        m.by_content(kinds, side, i);
        m
    }

    fn by_content(&mut self, kinds: &Kinds, side: Side, i: &mut dyn Inspect) {
        const LIMIT: usize = 10_000;
        let dels: Vec<&RelPath> = kinds.iter().filter(|(p, k)| **k == ChangeKind::Delete && !self.to.contains_key(*p)).map(|(p, _)| p).collect();
        let adds: Vec<&RelPath> = kinds.iter().filter(|(p, k)| **k == ChangeKind::Add && !self.from.contains_key(*p)).map(|(p, _)| p).collect();
        if dels.is_empty() || adds.is_empty() || dels.len() > LIMIT || adds.len() > LIMIT {
            return;
        }
        let files = |i: &mut dyn Inspect, side, ps: Vec<&RelPath>| -> Vec<(RelPath, u64)> {
            ps.into_iter().filter_map(|p| i.entry(side, p).filter(|e| e.ftype == FType::Reg).map(|e| (p.clone(), e.size))).collect()
        };
        let (dels, adds) = (files(i, Side::Base, dels), files(i, side, adds));
        let add_sizes: HashSet<u64> = adds.iter().map(|(_, n)| *n).collect();
        let mut left: HashMap<Vec<u8>, Option<RelPath>> = HashMap::new();
        for (q, _) in dels.iter().filter(|(_, n)| add_sizes.contains(n)) {
            left.entry(i.content(Side::Base, q)).and_modify(|e| *e = None).or_insert_with(|| Some(q.clone()));
        }
        let del_sizes: HashSet<u64> = dels.iter().map(|(_, n)| *n).collect();
        let mut arrived: HashMap<Vec<u8>, Option<RelPath>> = HashMap::new();
        for (p, _) in adds.iter().filter(|(_, n)| del_sizes.contains(n)) {
            let c = i.content(side, p);
            if left.contains_key(&c) {
                arrived.entry(c).and_modify(|e| *e = None).or_insert_with(|| Some(p.clone()));
            }
        }
        for (c, p) in arrived {
            if let (Some(p), Some(Some(q))) = (p, left.get(&c)) {
                self.to.insert(q.clone(), p.clone());
                self.from.insert(p, q.clone());
            }
        }
    }

    fn fwd(&self, q: &[u8]) -> RelPath {
        let mut a = q;
        while !a.is_empty() {
            if let Some(p) = self.to.get(a) {
                let mut r = p.clone();
                r.extend_from_slice(&q[a.len()..]);
                return r;
            }
            a = path::parent(a);
        }
        q.to_vec()
    }

    fn top(&self, q: &[u8]) -> bool {
        self.to.get(q).is_some_and(|p| path::file_name(p) != path::file_name(q) || self.fwd(path::parent(q)) != path::parent(p))
    }
}

struct Obj {
    q: RelPath,
    sp: Option<RelPath>,
    tp: Option<RelPath>,
    sc: bool,
    tc: bool,
    smv: bool,
    tmv: bool,
}

impl Obj {
    fn s_changed(&self) -> bool {
        self.sp.is_none() || self.sc || self.smv
    }
    fn t_changed(&self) -> bool {
        self.tp.is_none() || self.tc || self.tmv
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Who {
    Obj(usize),
    NewS(RelPath),
    NewT(RelPath),
}

struct Ctx<'a> {
    i: &'a mut dyn Inspect,
    rules: &'a Rules,
    resolutions: &'a BTreeMap<RelPath, Resolution>,
    src: Kinds,
    dst: Kinds,
    s: Moves,
    t: Moves,
    obj_memo: HashMap<(Side, RelPath), Option<RelPath>>,
    loc_memo: HashMap<RelPath, RelPath>,
    busy: HashSet<RelPath>,
    units: HashMap<String, Side>,
    used: BTreeSet<RelPath>,
    out: Plan,
}

impl Ctx<'_> {
    fn moves(&self, side: Side) -> &Moves {
        if side == Side::Source { &self.s } else { &self.t }
    }

    fn kinds(&self, side: Side) -> &Kinds {
        if side == Side::Source { &self.src } else { &self.dst }
    }

    fn conflict(&mut self, p: &[u8], reason: &'static str) {
        self.out.conflicts.push(Conflict { path: p.to_vec(), reason });
    }

    fn is_dir(&mut self, side: Side, p: &[u8]) -> bool {
        self.i.entry(side, p).is_some_and(|e| e.ftype == FType::Dir)
    }

    fn obj(&mut self, side: Side, d: &[u8]) -> Option<RelPath> {
        if d.is_empty() {
            return Some(Vec::new());
        }
        let key = (side, d.to_vec());
        if let Some(v) = self.obj_memo.get(&key) {
            return v.clone();
        }
        let v = match self.moves(side).from.get(d) {
            Some(q) => Some(q.clone()),
            None => self.obj(side, path::parent(d)).and_then(|po| {
                let q = path::join(&po, path::file_name(d));
                (!self.moves(side).to.contains_key(&q) && self.i.entry(Side::Base, &q).is_some()).then_some(q)
            }),
        };
        self.obj_memo.insert(key, v.clone());
        v
    }

    fn present(&mut self, side: Side, q: &[u8]) -> Option<RelPath> {
        let p = self.moves(side).fwd(q);
        (self.i.entry(side, &p).is_some() && self.obj(side, &p).as_deref() == Some(q)).then_some(p)
    }

    fn changed(&mut self, side: Side, q: &[u8], p: &[u8]) -> bool {
        if p == q {
            return self.kinds(side).get(q) == Some(&ChangeKind::Modify);
        }
        let (Some(a), Some(b)) = (self.i.entry(Side::Base, q), self.i.entry(side, p)) else { return true };
        if a != b || self.i.xattrs(Side::Base, q) != self.i.xattrs(side, p) {
            return true;
        }
        matches!(a.ftype, FType::Reg | FType::Symlink) && self.i.content(Side::Base, q) != self.i.content(side, p)
    }

    fn loc(&mut self, q: &[u8]) -> RelPath {
        if q.is_empty() {
            return Vec::new();
        }
        if let Some(r) = self.loc_memo.get(q) {
            return r.clone();
        }
        if !self.busy.insert(q.to_vec()) {
            self.conflict(q, "rename cycle");
            return q.to_vec();
        }
        let by = |cx: &mut Self, side: Side| -> Option<RelPath> {
            let p = cx.moves(side).to.get(q).filter(|_| cx.moves(side).top(q))?.clone();
            let parent = cx.place(side, path::parent(&p));
            Some(path::join(&parent, path::file_name(&p)))
        };
        let (rs, rt) = (by(self, Side::Source), by(self, Side::Target));
        let r = match (rs, rt) {
            (Some(a), Some(b)) if a != b => match self.resolve(&a) {
                Some(Resolution::Theirs) => a,
                Some(Resolution::Ours) => b,
                _ => {
                    self.conflict(&a, "rename/rename");
                    b
                }
            },
            (Some(a), _) | (None, Some(a)) => a,
            (None, None) => {
                let parent = self.loc(path::parent(q));
                path::join(&parent, path::file_name(q))
            }
        };
        self.busy.remove(q);
        self.loc_memo.insert(q.to_vec(), r.clone());
        r
    }

    fn place(&mut self, side: Side, d: &[u8]) -> RelPath {
        if d.is_empty() {
            return Vec::new();
        }
        match self.obj(side, d) {
            Some(q) => self.loc(&q),
            None => {
                let parent = self.place(side, path::parent(d));
                path::join(&parent, path::file_name(d))
            }
        }
    }

    fn resolve(&mut self, r: &[u8]) -> Option<Resolution> {
        let res = self.resolutions.get(r)?.clone();
        self.used.insert(r.to_vec());
        Some(res)
    }

    fn strategy(&mut self, r: &[u8], is_dir: bool) -> Option<Strategy> {
        match self.rules.strategy(r, is_dir)? {
            Strategy::Unit(u) => Some(match self.units.get(u)? {
                Side::Source => Strategy::Theirs,
                _ => Strategy::Ours,
            }),
            s => Some(s.clone()),
        }
    }

    fn newer(&mut self, s: &[u8], t: &[u8]) -> Side {
        if self.i.mtime(Side::Source, s) > self.i.mtime(Side::Target, t) { Side::Source } else { Side::Target }
    }

    fn decide(&mut self, r: &[u8], o: Option<&[u8]>, s: &[u8], t: &[u8]) -> Result<Option<Action>, &'static str> {
        let take = |cx: &mut Self| if cx.i.entry(Side::Source, s).is_some() { Some(Action::Write) } else { None };
        match self.resolve(r) {
            Some(Resolution::Ours) => return Ok(None),
            Some(Resolution::Theirs) => return Ok(take(self)),
            Some(Resolution::Content(c)) => return Ok(Some(Action::Content(c))),
            None => {}
        }
        let is_dir = self.is_dir(Side::Source, s) || self.is_dir(Side::Target, t);
        let union = match self.strategy(r, is_dir) {
            Some(Strategy::Ours) => return Ok(None),
            Some(Strategy::Theirs) => return Ok(take(self)),
            Some(Strategy::Newer) => return Ok(if self.newer(s, t) == Side::Source { take(self) } else { None }),
            Some(Strategy::Union) => true,
            _ => false,
        };
        both_changed(self.i, o, s, t, union)
    }

    fn decide_delete(&mut self, r: &[u8], deleted_by: Side, s: &[u8], t: &[u8], reason: &'static str) -> Result<bool, &'static str> {
        let theirs = deleted_by == Side::Source;
        match self.resolve(r) {
            Some(Resolution::Ours) => return Ok(!theirs),
            Some(Resolution::Theirs) => return Ok(theirs),
            Some(Resolution::Content(_)) => return Err("content resolution for a deleted entry"),
            None => {}
        }
        let is_dir = self.is_dir(Side::Source, s) || self.is_dir(Side::Target, t);
        match self.strategy(r, is_dir) {
            Some(Strategy::Ours) => Ok(!theirs),
            Some(Strategy::Theirs) => Ok(theirs),
            Some(Strategy::Newer) => Ok((self.newer(s, t) == Side::Source) == theirs),
            _ => Err(reason),
        }
    }

    fn write(&mut self, r: RelPath, action: Action, o: Option<&RelPath>, s: Option<&RelPath>, t: Option<&RelPath>) {
        self.out.writes.insert(r, Step { action, o: o.cloned(), s: s.cloned(), t: t.cloned() });
    }

    fn decide_units(&mut self, objs: &[Obj], news_s: &[RelPath], news_t: &[RelPath]) {
        if self.rules.is_empty() {
            return;
        }
        let rules = self.rules;
        let mut newest: HashMap<String, [Option<(i64, i64)>; 2]> = HashMap::new();
        let mut note = |cx: &mut Self, r: &[u8], is_dir: bool, side: Side, p: &[u8]| {
            if let Some(Strategy::Unit(u)) = rules.strategy(r, is_dir) {
                let m = cx.i.mtime(side, p);
                let slot = &mut newest.entry(u.clone()).or_default()[(side == Side::Target) as usize];
                *slot = (*slot).max(Some(m));
            }
        };
        for o in objs {
            let r = self.loc(&o.q);
            let is_dir = self.is_dir(Side::Base, &o.q);
            if o.s_changed() {
                let p = o.sp.clone().unwrap_or_else(|| self.s.fwd(&o.q));
                note(self, &r, is_dir, Side::Source, &p);
            }
            if o.t_changed() {
                let p = o.tp.clone().unwrap_or_else(|| self.t.fwd(&o.q));
                note(self, &r, is_dir, Side::Target, &p);
            }
        }
        for (side, news) in [(Side::Source, news_s), (Side::Target, news_t)] {
            for p in news {
                let r = self.place(side, p);
                let is_dir = self.is_dir(side, p);
                note(self, &r, is_dir, side, p);
            }
        }
        for (u, [s, t]) in newest {
            if let (Some(s), Some(t)) = (s, t) {
                self.units.insert(u, if s > t { Side::Source } else { Side::Target });
            }
        }
    }

    fn decide_obj(&mut self, o: &Obj, r: &[u8]) -> bool {
        let is_dir = self.is_dir(Side::Base, &o.q);
        let unit = match self.rules.strategy(r, is_dir) {
            Some(Strategy::Unit(u)) => self.units.get(u).copied(),
            _ => None,
        };
        let (sp, tp) = (o.sp.as_ref(), o.tp.as_ref());
        if let Some(side) = unit {
            if side == Side::Target {
                return tp.is_some();
            }
            match (sp, tp) {
                (None, Some(tp)) => {
                    self.out.deletes.insert(tp.clone());
                    return false;
                }
                (Some(sp), _) if o.sc || tp.is_none() || o.tc => self.write(r.to_vec(), Action::Write, Some(&o.q), Some(sp), tp),
                _ => {}
            }
            return sp.is_some();
        }
        match (sp, tp) {
            (None, None) => false,
            (None, Some(tp)) => {
                if !o.t_changed() {
                    self.out.deletes.insert(tp.clone());
                    return false;
                }
                let reason = if o.tc { "delete/modify" } else { "rename/delete" };
                let s = self.s.fwd(&o.q);
                match self.decide_delete(r, Side::Source, &s, tp, reason) {
                    Ok(true) => {
                        self.out.deletes.insert(tp.clone());
                        false
                    }
                    Ok(false) => true,
                    Err(reason) => {
                        self.conflict(r, reason);
                        true
                    }
                }
            }
            (Some(sp), None) => {
                if !o.s_changed() {
                    return false;
                }
                let reason = if o.sc { "delete/modify" } else { "rename/delete" };
                let t = self.t.fwd(&o.q);
                match self.decide_delete(r, Side::Target, sp, &t, reason) {
                    Ok(true) => false,
                    Ok(false) => {
                        self.write(r.to_vec(), Action::Write, Some(&o.q), Some(sp), None);
                        true
                    }
                    Err(reason) => {
                        self.conflict(r, reason);
                        false
                    }
                }
            }
            (Some(sp), Some(tp)) => {
                match (o.sc, o.tc) {
                    (false, _) => {}
                    (true, false) => self.write(r.to_vec(), Action::Write, Some(&o.q), Some(sp), Some(tp)),
                    (true, true) => match self.decide(r, Some(&o.q), sp, tp) {
                        Ok(Some(a)) => self.write(r.to_vec(), a, Some(&o.q), Some(sp), Some(tp)),
                        Ok(None) => {}
                        Err(reason) => self.conflict(r, reason),
                    },
                }
                true
            }
        }
    }

    fn t_of(&mut self, r: &[u8], landing: &BTreeMap<RelPath, Vec<Who>>, objs: &[Obj], qs: &HashSet<&RelPath>, gone: &BTreeSet<RelPath>) -> Option<RelPath> {
        if r.is_empty() {
            return Some(Vec::new());
        }
        if let Some(ws) = landing.get(r) {
            return ws.iter().find_map(|w| match w {
                Who::Obj(k) => objs[*k].tp.clone(),
                Who::NewT(p) => Some(p.clone()),
                Who::NewS(_) => None,
            });
        }
        let tp = path::join(&self.t_of(path::parent(r), landing, objs, qs, gone)?, path::file_name(r));
        if self.i.entry(Side::Target, &tp).is_none() || gone.contains(&tp) {
            return None;
        }
        let decided = match self.obj(Side::Target, &tp) {
            Some(q) => qs.contains(&q),
            None => self.dst.contains_key(&tp),
        };
        (!decided).then_some(tp)
    }

    fn run(&mut self, idents: (bool, bool)) {
        self.filter_ignored();
        self.s = Moves::detect(&self.src, Side::Source, idents.0, self.i);
        self.t = Moves::detect(&self.dst, Side::Target, idents.1, self.i);
        let mut qs = BTreeSet::new();
        let (mut news_s, mut news_t) = (Vec::new(), Vec::new());
        for side in [Side::Source, Side::Target] {
            let kinds: Vec<(RelPath, ChangeKind)> = self.kinds(side).iter().map(|(p, k)| (p.clone(), *k)).collect();
            for (p, k) in kinds {
                if k != ChangeKind::Add {
                    qs.insert(p.clone());
                }
                if k != ChangeKind::Delete {
                    match self.obj(side, &p) {
                        Some(q) => {
                            qs.insert(q);
                        }
                        None if side == Side::Source => news_s.push(p),
                        None => news_t.push(p),
                    }
                }
            }
        }
        let mut objs = Vec::new();
        for q in qs {
            let sp = self.present(Side::Source, &q);
            let tp = self.present(Side::Target, &q);
            let sc = sp.as_ref().is_some_and(|p| self.changed(Side::Source, &q, &p.clone()));
            let tc = tp.as_ref().is_some_and(|p| self.changed(Side::Target, &q, &p.clone()));
            let smv = sp.is_some() && self.s.top(&q);
            let tmv = tp.is_some() && self.t.top(&q);
            objs.push(Obj { q, sp, tp, sc, tc, smv, tmv });
        }
        self.decide_units(&objs, &news_s, &news_t);

        let mut landing: BTreeMap<RelPath, Vec<Who>> = BTreeMap::new();
        for (k, o) in objs.iter().enumerate() {
            let r = self.loc(&o.q);
            if self.decide_obj(o, &r) {
                landing.entry(r).or_default().push(Who::Obj(k));
            }
        }
        for (side, news) in [(Side::Target, &news_t), (Side::Source, &news_s)] {
            for p in news {
                let r = self.place(side, p);
                let is_dir = self.is_dir(side, p);
                let by = match self.rules.strategy(&r, is_dir) {
                    Some(Strategy::Unit(u)) => self.units.get(u).copied(),
                    _ => None,
                };
                match (side, by) {
                    (Side::Target, Some(Side::Source)) => {
                        self.out.deletes.insert(p.clone());
                    }
                    (Side::Source, Some(Side::Target)) => {}
                    (Side::Target, _) => landing.entry(r).or_default().push(Who::NewT(p.clone())),
                    _ => landing.entry(r).or_default().push(Who::NewS(p.clone())),
                }
            }
        }

        for (r, ws) in &landing {
            match ws.as_slice() {
                [Who::NewS(p)] => self.write(r.clone(), Action::Write, None, Some(p), None),
                [_] => {}
                [a, b] if matches!((a, b), (Who::NewS(_), _) | (_, Who::NewS(_))) => {
                    let ((Who::NewS(sp), other) | (other, Who::NewS(sp))) = (a, b) else { unreachable!() };
                    let tp = match other {
                        Who::Obj(k) if objs[*k].smv => None,
                        Who::Obj(k) => objs[*k].tp.clone(),
                        Who::NewT(p) => Some(p.clone()),
                        Who::NewS(_) => None,
                    };
                    let Some(tp) = tp else {
                        self.conflict(r, "add/add");
                        continue;
                    };
                    match self.decide(r, None, sp, &tp) {
                        Ok(Some(a)) => self.write(r.clone(), a, None, Some(sp), Some(&tp)),
                        Ok(None) => {}
                        Err(reason) => self.conflict(r, reason),
                    }
                }
                _ => self.conflict(r, "rename/add"),
            }
        }

        let gone = self.out.deletes.clone();
        for o in &objs {
            let Some(tp) = &o.tp else { continue };
            if gone.contains(tp) {
                continue;
            }
            let r = self.loc(&o.q);
            let parent = self.place(Side::Target, path::parent(tp));
            if path::join(&parent, path::file_name(tp)) != r {
                self.out.moves.push(Move { from: tp.clone(), to: r });
            }
        }
        self.structure(&objs, &landing, &gone);
    }

    fn filter_ignored(&mut self) {
        if self.rules.is_empty() {
            return;
        }
        for side in [Side::Source, Side::Target] {
            let kinds = std::mem::take(if side == Side::Source { &mut self.src } else { &mut self.dst });
            let kept = kinds
                .into_iter()
                .filter(|(p, k)| {
                    let is_dir = self.is_dir(if *k == ChangeKind::Delete { Side::Base } else { side }, p);
                    !self.rules.ignored(p, is_dir)
                })
                .collect();
            *(if side == Side::Source { &mut self.src } else { &mut self.dst }) = kept;
        }
    }

    fn structure(&mut self, objs: &[Obj], landing: &BTreeMap<RelPath, Vec<Who>>, gone: &BTreeSet<RelPath>) {
        let qs: HashSet<&RelPath> = objs.iter().map(|o| &o.q).collect();
        let mut targets: Vec<RelPath> = self.out.writes.keys().cloned().collect();
        targets.extend(self.out.moves.iter().map(|m| m.to.clone()));
        for r in targets {
            if let Some(Step { action: Action::Write, s: Some(s), .. }) = self.out.writes.get(&r)
                && self.i.entry(Side::Source, s).is_some_and(|e| e.ftype == FType::Other)
            {
                self.conflict(&r, "unsupported file type");
            }
            for anc in path::ancestors(&r) {
                let reason = match self.out.writes.get(anc) {
                    Some(Step { action: Action::Write, s: Some(s), .. }) => {
                        let s = s.clone();
                        (!self.is_dir(Side::Source, &s)).then_some("parent type")
                    }
                    Some(Step { action: Action::Meta, .. }) | None => match self.t_of(anc, landing, objs, &qs, gone) {
                        Some(tp) if self.is_dir(Side::Target, &tp) => None,
                        Some(_) => Some("parent type"),
                        None => Some("parent deleted in target"),
                    },
                    Some(_) => Some("parent type"),
                };
                if let Some(reason) = reason {
                    self.conflict(&r, reason);
                    break;
                }
            }
        }
        let mut removed: Vec<(RelPath, RelPath)> = Vec::new();
        for o in objs {
            if let Some(tp) = &o.tp
                && gone.contains(tp)
                && self.is_dir(Side::Target, tp)
            {
                removed.push((tp.clone(), self.loc(&o.q)));
            }
        }
        let replaced: Vec<(RelPath, RelPath)> = self
            .out
            .writes
            .iter()
            .filter_map(|(r, st)| {
                let t = st.t.clone()?;
                let s_dir = st.s.as_ref().is_some_and(|s| self.i.entry(Side::Source, s).is_some_and(|e| e.ftype == FType::Dir));
                let replaces = match st.action {
                    Action::Write => !s_dir,
                    Action::Content(_) => true,
                    _ => false,
                };
                (replaces && self.i.entry(Side::Target, &t).is_some_and(|e| e.ftype == FType::Dir)).then(|| (t, r.clone()))
            })
            .collect();
        removed.extend(replaced);
        if removed.is_empty() {
            return;
        }
        let changed: Vec<RelPath> = self.dst.iter().filter(|(_, k)| **k != ChangeKind::Delete).map(|(p, _)| p.clone()).collect();
        for (tdir, rdir) in removed {
            for x in changed.iter().filter(|x| path::is_under(x, &tdir)) {
                if gone.iter().any(|g| (g == x || path::is_under(x, g)) && path::is_under(g, &tdir)) {
                    continue;
                }
                let end = match self.obj(Side::Target, x) {
                    Some(q) => self.loc(&q),
                    None => self.place(Side::Target, x),
                };
                if end == rdir || path::is_under(&end, &rdir) {
                    self.conflict(&rdir, "delete/modify (subtree)");
                    break;
                }
            }
        }
    }
}

fn both_changed(i: &mut dyn Inspect, o: Option<&[u8]>, s: &[u8], t: &[u8], union: bool) -> Result<Option<Action>, &'static str> {
    let (se, te) = match (i.entry(Side::Source, s), i.entry(Side::Target, t)) {
        (None, None) => return Ok(None),
        (Some(a), Some(b)) => (a, b),
        _ => return Err("delete/modify"),
    };
    if se.ftype != te.ftype {
        return Err("type");
    }
    let o = o.filter(|o| i.entry(Side::Base, o).is_some_and(|e| e.ftype == se.ftype));
    let has_content = matches!(se.ftype, FType::Reg | FType::Symlink);
    let mut content = |side, p: &[u8]| if has_content { i.content(side, p) } else { Vec::new() };
    let (sc, tc) = (content(Side::Source, s), content(Side::Target, t));
    let s_meta = (se.meta, i.xattrs(Side::Source, s));
    let t_meta = (te.meta, i.xattrs(Side::Target, t));
    let o_meta = o.map(|o| (i.entry(Side::Base, o).unwrap().meta, i.xattrs(Side::Base, o)));
    let meta = if s_meta == t_meta {
        Some(Side::Target)
    } else {
        match &o_meta {
            Some(om) if *om == t_meta => Some(Side::Source),
            Some(om) if *om == s_meta => Some(Side::Target),
            _ => None,
        }
    };
    if sc == tc {
        return match meta {
            None => Err("metadata"),
            Some(Side::Source) => Ok(Some(Action::Meta)),
            _ => Ok(None),
        };
    }
    let Some(o) = o else { return Err(if se.ftype == FType::Reg { "add/add" } else { "symlink" }) };
    let Some(meta_from) = meta else { return Err("metadata") };
    let oc = if has_content { i.content(Side::Base, o) } else { Vec::new() };
    if tc == oc {
        return Ok(Some(if meta_from == Side::Source || s_meta == t_meta { Action::Write } else { Action::WriteContent }));
    }
    if sc == oc {
        return Ok((meta_from == Side::Source).then_some(Action::Meta));
    }
    if se.ftype != FType::Reg {
        return Err("symlink");
    }
    if !(i.is_text(Side::Source, s) && i.is_text(Side::Target, t) && i.is_text(Side::Base, o)) {
        return Err("binary");
    }
    Ok(Some(Action::TextMerge { meta_from, union }))
}

pub fn plan(input: &Input, i: &mut dyn Inspect) -> Plan {
    let mut cx = Ctx {
        i,
        rules: input.rules,
        resolutions: input.resolutions,
        src: input.source.clone(),
        dst: input.target.clone(),
        s: Moves::default(),
        t: Moves::default(),
        obj_memo: HashMap::new(),
        loc_memo: HashMap::new(),
        busy: HashSet::new(),
        units: HashMap::new(),
        used: BTreeSet::new(),
        out: Plan::default(),
    };
    cx.run(input.idents);
    let mut out = cx.out;
    out.unused_resolutions = input.resolutions.keys().filter(|k| !cx.used.contains(*k)).cloned().collect();
    out.conflicts.sort_by(|a, b| a.path.cmp(&b.path));
    out.conflicts.dedup_by(|a, b| a.path == b.path);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone)]
    struct E {
        t: FType,
        mode: u32,
        data: &'static [u8],
        ino: u64,
        mtime: i64,
    }

    #[derive(Default)]
    struct Fake {
        o: HashMap<RelPath, E>,
        s: HashMap<RelPath, E>,
        t: HashMap<RelPath, E>,
    }

    impl Fake {
        fn get(&self, side: Side, p: &[u8]) -> Option<&E> {
            match side {
                Side::Base => &self.o,
                Side::Source => &self.s,
                Side::Target => &self.t,
            }
            .get(p)
        }
    }

    impl Inspect for Fake {
        fn entry(&mut self, side: Side, p: &[u8]) -> Option<Entry> {
            if p.is_empty() {
                return Some(Entry { ftype: FType::Dir, meta: Meta { mode: 0o755, uid: 1000, gid: 1000 }, size: 0 });
            }
            self.get(side, p).map(|e| Entry {
                ftype: e.t,
                meta: Meta { mode: e.mode, uid: 1000, gid: 1000 },
                size: if e.t == FType::Reg { e.data.len() as u64 } else { 0 },
            })
        }
        fn content(&mut self, side: Side, p: &[u8]) -> Vec<u8> {
            self.get(side, p).unwrap().data.to_vec()
        }
        fn is_text(&mut self, side: Side, p: &[u8]) -> bool {
            !self.get(side, p).unwrap().data.contains(&0)
        }
        fn xattrs(&mut self, _: Side, _: &[u8]) -> Vec<(Vec<u8>, Vec<u8>)> {
            Vec::new()
        }
        fn ident(&mut self, side: Side, p: &[u8]) -> Option<u64> {
            self.get(side, p).map(|e| e.ino).filter(|i| *i != 0)
        }
        fn mtime(&mut self, side: Side, p: &[u8]) -> (i64, i64) {
            let mut p = p;
            loop {
                if let Some(e) = self.get(side, p) {
                    return (e.mtime, 0);
                }
                if p.is_empty() {
                    return (0, 0);
                }
                p = path::parent(p);
            }
        }
    }

    fn f(data: &'static [u8]) -> E {
        E { t: FType::Reg, mode: 0o644, data, ino: 0, mtime: 0 }
    }
    fn d() -> E {
        E { t: FType::Dir, mode: 0o755, data: b"", ino: 0, mtime: 0 }
    }
    fn id(e: E, ino: u64) -> E {
        E { ino, ..e }
    }
    fn at(e: E, mtime: i64) -> E {
        E { mtime, ..e }
    }
    fn ch(items: &[(&str, ChangeKind)]) -> Kinds {
        items.iter().map(|(p, k)| (p.as_bytes().to_vec(), *k)).collect()
    }
    fn set(m: &mut HashMap<RelPath, E>, items: &[(&str, E)]) {
        for (p, e) in items {
            m.insert(p.as_bytes().to_vec(), e.clone());
        }
    }
    fn reasons(p: &Plan) -> Vec<(String, &'static str)> {
        p.conflicts.iter().map(|c| (String::from_utf8_lossy(&c.path).into_owned(), c.reason)).collect()
    }
    fn s(p: &str) -> RelPath {
        p.as_bytes().to_vec()
    }
    fn moves(p: &Plan) -> Vec<(String, String)> {
        p.moves.iter().map(|m| (String::from_utf8_lossy(&m.from).into_owned(), String::from_utf8_lossy(&m.to).into_owned())).collect()
    }
    fn writes(p: &Plan) -> Vec<(String, Action)> {
        p.writes.iter().map(|(r, st)| (String::from_utf8_lossy(r).into_owned(), st.action.clone())).collect()
    }

    struct Run<'a> {
        src: Kinds,
        dst: Kinds,
        idents: (bool, bool),
        res: BTreeMap<RelPath, Resolution>,
        rules: &'a Rules,
    }

    fn run(x: &mut Fake, src: Kinds, dst: Kinds) -> Plan {
        go(x, Run { src, dst, idents: (true, true), res: BTreeMap::new(), rules: &Rules::default() })
    }

    fn go(x: &mut Fake, r: Run) -> Plan {
        plan(&Input { source: &r.src, target: &r.dst, idents: r.idents, resolutions: &r.res, rules: r.rules }, x)
    }

    use ChangeKind::{Add, Delete, Modify};

    #[test]
    fn one_sided_and_text() {
        let mut x = Fake::default();
        set(&mut x.o, &[("a", f(b"1\n")), ("t", f(b"x\n"))]);
        set(&mut x.s, &[("a", f(b"2\n")), ("n", f(b"new")), ("t", f(b"x\ns\n"))]);
        set(&mut x.t, &[("a", f(b"1\n")), ("t", f(b"t\nx\n"))]);
        let p = run(&mut x, ch(&[("a", Modify), ("n", Add), ("t", Modify)]), ch(&[("t", Modify)]));
        assert!(p.conflicts.is_empty());
        assert_eq!(
            writes(&p),
            vec![("a".into(), Action::Write), ("n".into(), Action::Write), ("t".into(), Action::TextMerge { meta_from: Side::Target, union: false })]
        );
        assert!(p.deletes.is_empty() && p.moves.is_empty());
    }

    #[test]
    fn conflicts_and_resolutions() {
        let mut x = Fake::default();
        set(&mut x.o, &[("bin", f(b"\0a")), ("dm", f(b"x")), ("ty", f(b"x"))]);
        set(&mut x.s, &[("bin", f(b"\0b")), ("ty", d()), ("aa", f(b"s"))]);
        set(&mut x.t, &[("bin", f(b"\0c")), ("dm", f(b"y")), ("ty", f(b"z")), ("aa", f(b"t"))]);
        let src = ch(&[("bin", Modify), ("dm", Delete), ("ty", Modify), ("aa", Add)]);
        let dst = ch(&[("bin", Modify), ("dm", Modify), ("ty", Modify), ("aa", Add)]);
        let p = run(&mut x, src.clone(), dst.clone());
        assert_eq!(reasons(&p), vec![("aa".into(), "add/add"), ("bin".into(), "binary"), ("dm".into(), "delete/modify"), ("ty".into(), "type")]);
        let res: BTreeMap<RelPath, Resolution> = [
            (s("aa"), Resolution::Ours),
            (s("bin"), Resolution::Theirs),
            (s("dm"), Resolution::Theirs),
            (s("ty"), Resolution::Content(b"c".to_vec())),
            (s("zz"), Resolution::Ours),
        ]
        .into();
        let p = go(&mut x, Run { src, dst, idents: (true, true), res, rules: &Rules::default() });
        assert!(p.conflicts.is_empty(), "{:?}", p.conflicts);
        assert_eq!(writes(&p), vec![("bin".into(), Action::Write), ("ty".into(), Action::Content(b"c".to_vec()))]);
        assert_eq!(p.deletes, [s("dm")].into());
        assert_eq!(p.unused_resolutions, vec![s("zz")]);
    }

    #[test]
    fn structural() {
        let mut x = Fake::default();
        set(&mut x.o, &[("d", d()), ("d/a", f(b"a")), ("e", d()), ("e/x", f(b"x"))]);
        set(&mut x.s, &[("d", d()), ("d/a", f(b"a")), ("d/new", f(b"n"))]);
        set(&mut x.t, &[("d", E { t: FType::Symlink, mode: 0o777, data: b"/etc", ino: 0, mtime: 0 }), ("e", d()), ("e/x", f(b"x")), ("e/tnew", f(b"t"))]);
        let src = ch(&[("d/new", Add), ("e", Delete), ("e/x", Delete)]);
        let dst = ch(&[("d", Modify), ("d/a", Delete), ("e/tnew", Add)]);
        let p = run(&mut x, src, dst);
        assert_eq!(reasons(&p), vec![("d/new".into(), "parent type"), ("e".into(), "delete/modify (subtree)")]);
        let mut y = Fake::default();
        set(&mut y.o, &[("g", d())]);
        set(&mut y.s, &[("g", d()), ("g/n", f(b"n"))]);
        let p = run(&mut y, ch(&[("g/n", Add)]), ch(&[("g", Delete)]));
        assert_eq!(reasons(&p), vec![("g/n".into(), "parent deleted in target")]);
        let mut z = Fake::default();
        set(&mut z.s, &[("dev", E { t: FType::Other, mode: 0o600, data: b"", ino: 0, mtime: 0 })]);
        let p = run(&mut z, ch(&[("dev", Add)]), BTreeMap::new());
        assert_eq!(reasons(&p), vec![("dev".into(), "unsupported file type")]);
    }

    #[test]
    fn metadata_merge() {
        let mut x = Fake::default();
        let m = E { mode: 0o600, ..f(b"x") };
        set(&mut x.o, &[("a", f(b"x")), ("b", f(b"x")), ("bin", f(b"\0x")), ("bin2", f(b"\0x"))]);
        set(&mut x.s, &[("a", m.clone()), ("b", m.clone()), ("bin", f(b"\0s")), ("bin2", E { mode: 0o600, ..f(b"\0x") })]);
        set(&mut x.t, &[("a", f(b"x")), ("b", E { mode: 0o700, ..f(b"x") }), ("bin", E { mode: 0o600, ..f(b"\0x") }), ("bin2", f(b"\0t"))]);
        let both = ch(&[("a", Modify), ("b", Modify), ("bin", Modify), ("bin2", Modify)]);
        let p = run(&mut x, both.clone(), both);
        assert_eq!(reasons(&p), vec![("b".into(), "metadata")]);
        assert_eq!(writes(&p), vec![("a".into(), Action::Meta), ("bin".into(), Action::WriteContent), ("bin2".into(), Action::Meta)]);
    }

    fn tree(x: &mut Fake) {
        for m in [&mut x.o, &mut x.s, &mut x.t] {
            set(m, &[("a", id(f(b"a\n"), 10)), ("d", id(d(), 20)), ("d/x", id(f(b"x\n"), 21)), ("d/y", id(f(b"y\n"), 22))]);
        }
    }
    fn mv(m: &mut HashMap<RelPath, E>, from: &str, to: &str) {
        let keys: Vec<RelPath> = m.keys().filter(|k| *k == from.as_bytes() || path::is_under(k, from.as_bytes())).cloned().collect();
        for k in keys {
            let e = m.remove(&k).unwrap();
            let mut n = to.as_bytes().to_vec();
            n.extend_from_slice(&k[from.len()..]);
            m.insert(n, e);
        }
    }

    #[test]
    fn edit_follows_rename() {
        let mut x = Fake::default();
        tree(&mut x);
        mv(&mut x.s, "a", "b");
        set(&mut x.t, &[("a", id(f(b"a\nt\n"), 10))]);
        let p = run(&mut x, ch(&[("a", Delete), ("b", Add)]), ch(&[("a", Modify)]));
        assert!(p.conflicts.is_empty(), "{:?}", p.conflicts);
        assert_eq!(moves(&p), vec![("a".into(), "b".into())]);
        assert!(p.writes.is_empty());
        set(&mut x.s, &[("b", id(f(b"s\na\n"), 10))]);
        let p = run(&mut x, ch(&[("a", Delete), ("b", Add)]), ch(&[("a", Modify)]));
        assert_eq!(moves(&p), vec![("a".into(), "b".into())]);
        assert_eq!(writes(&p), vec![("b".into(), Action::TextMerge { meta_from: Side::Target, union: false })]);
        let st = &p.writes[&s("b")];
        assert_eq!((st.o.as_deref(), st.s.as_deref(), st.t.as_deref()), (Some(&b"a"[..]), Some(&b"b"[..]), Some(&b"a"[..])));
        let p = go(
            &mut x,
            Run { src: ch(&[("a", Delete), ("b", Add)]), dst: ch(&[("a", Modify)]), idents: (false, true), res: BTreeMap::new(), rules: &Rules::default() },
        );
        assert_eq!(reasons(&p), vec![("a".into(), "delete/modify")]);
        set(&mut x.s, &[("b", id(f(b"a\n"), 55))]);
        for idents in [(false, true), (true, true)] {
            let r = Run { src: ch(&[("a", Delete), ("b", Add)]), dst: ch(&[("a", Modify)]), idents, res: BTreeMap::new(), rules: &Rules::default() };
            let p = go(&mut x, r);
            assert!(p.conflicts.is_empty(), "{:?}", p.conflicts);
            assert_eq!(moves(&p), vec![("a".into(), "b".into())]);
        }
    }

    #[test]
    fn change_follows_target_rename() {
        let mut x = Fake::default();
        tree(&mut x);
        set(&mut x.s, &[("a", id(f(b"a\ns\n"), 10)), ("d/new", id(f(b"n"), 90))]);
        x.s.remove(&s("d/y"));
        mv(&mut x.t, "a", "b");
        mv(&mut x.t, "d", "e");
        let src = ch(&[("a", Modify), ("d/new", Add), ("d/y", Delete)]);
        let dst = ch(&[("a", Delete), ("b", Add), ("d", Delete), ("d/x", Delete), ("d/y", Delete), ("e", Add), ("e/x", Add), ("e/y", Add)]);
        let p = run(&mut x, src, dst);
        assert!(p.conflicts.is_empty(), "{:?}", p.conflicts);
        assert!(p.moves.is_empty());
        assert_eq!(writes(&p), vec![("b".into(), Action::Write), ("e/new".into(), Action::Write)]);
        assert_eq!(p.deletes, [s("e/y")].into());
    }

    #[test]
    fn directory_rename_carries_target_changes() {
        let mut x = Fake::default();
        tree(&mut x);
        mv(&mut x.s, "d", "e");
        set(&mut x.t, &[("d/x", id(f(b"x\nt\n"), 21)), ("d/new", id(f(b"n"), 91))]);
        let src = ch(&[("d", Delete), ("d/x", Delete), ("d/y", Delete), ("e", Add), ("e/x", Add), ("e/y", Add)]);
        let dst = ch(&[("d/x", Modify), ("d/new", Add)]);
        let p = run(&mut x, src, dst);
        assert!(p.conflicts.is_empty(), "{:?}", p.conflicts);
        assert_eq!(moves(&p), vec![("d".into(), "e".into())]);
        assert!(p.writes.is_empty() && p.deletes.is_empty());
        mv(&mut x.s, "e/x", "x2");
        let src = ch(&[("d", Delete), ("d/x", Delete), ("d/y", Delete), ("e", Add), ("e/y", Add), ("x2", Add)]);
        let p = run(&mut x, src, ch(&[("d/x", Modify), ("d/new", Add)]));
        assert!(p.conflicts.is_empty(), "{:?}", p.conflicts);
        let mut m = moves(&p);
        m.sort();
        assert_eq!(m, vec![("d".into(), "e".into()), ("d/x".into(), "x2".into())]);
    }

    #[test]
    fn rename_conflicts() {
        let both = |x: &mut Fake, res: BTreeMap<RelPath, Resolution>| {
            go(x, Run { src: ch(&[("a", Delete), ("b", Add)]), dst: ch(&[("a", Delete), ("c", Add)]), idents: (true, true), res, rules: &Rules::default() })
        };
        let mut x = Fake::default();
        tree(&mut x);
        mv(&mut x.s, "a", "b");
        mv(&mut x.t, "a", "c");
        assert_eq!(reasons(&both(&mut x, BTreeMap::new())), vec![("b".into(), "rename/rename")]);
        let p = both(&mut x, [(s("b"), Resolution::Theirs)].into());
        assert!(p.conflicts.is_empty() && p.unused_resolutions.is_empty());
        assert_eq!(moves(&p), vec![("c".into(), "b".into())]);
        let p = both(&mut x, [(s("b"), Resolution::Ours)].into());
        assert!(p.conflicts.is_empty() && p.moves.is_empty());
        let mut y = Fake::default();
        tree(&mut y);
        mv(&mut y.s, "a", "b");
        mv(&mut y.t, "a", "b");
        let p = run(&mut y, ch(&[("a", Delete), ("b", Add)]), ch(&[("a", Delete), ("b", Add)]));
        assert!(p.conflicts.is_empty() && p.is_empty(), "{:?} {:?}", p.conflicts, moves(&p));

        let mut z = Fake::default();
        tree(&mut z);
        mv(&mut z.s, "a", "b");
        z.t.remove(&s("a"));
        assert_eq!(reasons(&run(&mut z, ch(&[("a", Delete), ("b", Add)]), ch(&[("a", Delete)]))), vec![("b".into(), "rename/delete")]);
        set(&mut z.t, &[("a", id(f(b"a\n"), 10)), ("b", id(f(b"other"), 95))]);
        assert_eq!(reasons(&run(&mut z, ch(&[("a", Delete), ("b", Add)]), ch(&[("b", Add)]))), vec![("b".into(), "rename/add")]);
    }

    #[test]
    fn swap_and_cycle() {
        let mut x = Fake::default();
        tree(&mut x);
        let (ea, ex) = (x.s.remove(&s("a")).unwrap(), x.s.remove(&s("d/x")).unwrap());
        x.s.insert(s("a"), ex);
        x.s.insert(s("d/x"), ea);
        set(&mut x.t, &[("a", id(f(b"a\nt\n"), 10))]);
        let p = run(&mut x, ch(&[("a", Modify), ("d/x", Modify)]), ch(&[("a", Modify)]));
        assert!(p.conflicts.is_empty(), "{:?}", p.conflicts);
        let mut m = moves(&p);
        m.sort();
        assert_eq!(m, vec![("a".into(), "d/x".into()), ("d/x".into(), "a".into())]);
        assert!(p.writes.is_empty());
        let mut y = Fake::default();
        tree(&mut y);
        set(&mut y.s, &[("a", id(f(b"s\na\n"), 77))]);
        set(&mut y.t, &[("a", id(f(b"a\nt\n"), 10))]);
        let p = run(&mut y, ch(&[("a", Modify)]), ch(&[("a", Modify)]));
        assert!(p.conflicts.is_empty() && p.moves.is_empty());
        assert_eq!(writes(&p), vec![("a".into(), Action::TextMerge { meta_from: Side::Target, union: false })]);
        let mut z = Fake::default();
        tree(&mut z);
        for m in [&mut z.o, &mut z.s, &mut z.t] {
            set(m, &[("e", id(d(), 30))]);
        }
        mv(&mut z.s, "d", "e/d");
        mv(&mut z.t, "e", "d/e");
        let src = ch(&[("d", Delete), ("d/x", Delete), ("d/y", Delete), ("e/d", Add), ("e/d/x", Add), ("e/d/y", Add)]);
        let dst = ch(&[("e", Delete), ("d/e", Add)]);
        assert!(reasons(&run(&mut z, src, dst)).iter().any(|(_, r)| *r == "rename cycle"));
    }

    #[test]
    fn merge_rules() {
        let (rules, w) =
            Rules::parse(Some(b"target/\n"), Some(b"*.bin newer\nNOTES union\nCargo.toml unit=cargo\nCargo.lock unit=cargo\nkeep ours\nmine theirs\n"));
        assert!(w.is_empty());
        let mut x = Fake::default();
        set(&mut x.o, &[("a.bin", f(b"\0o")), ("NOTES", f(b"1\n")), ("Cargo.toml", f(b"t0")), ("Cargo.lock", f(b"l0")), ("keep", f(b"k")), ("mine", f(b"m"))]);
        set(
            &mut x.s,
            &[
                ("a.bin", at(f(b"\0s"), 5)),
                ("NOTES", f(b"1\ns\n")),
                ("Cargo.toml", at(f(b"t1"), 9)),
                ("Cargo.lock", f(b"l0")),
                ("keep", f(b"ks")),
                ("mine", f(b"ms")),
            ],
        );
        set(&mut x.s, &[("target", d()), ("target/out", f(b"so"))]);
        set(
            &mut x.t,
            &[
                ("a.bin", at(f(b"\0t"), 7)),
                ("NOTES", f(b"1\nt\n")),
                ("Cargo.toml", f(b"t0")),
                ("Cargo.lock", at(f(b"l2"), 3)),
                ("keep", f(b"kt")),
                ("mine", f(b"mt")),
            ],
        );
        let src = ch(&[("a.bin", Modify), ("NOTES", Modify), ("Cargo.toml", Modify), ("keep", Modify), ("mine", Modify), ("target", Add), ("target/out", Add)]);
        let dst = ch(&[("a.bin", Modify), ("NOTES", Modify), ("Cargo.lock", Modify), ("keep", Modify), ("mine", Modify)]);
        let p = go(&mut x, Run { src: src.clone(), dst: dst.clone(), idents: (true, true), res: BTreeMap::new(), rules: &rules });
        assert!(p.conflicts.is_empty(), "{:?}", p.conflicts);
        assert_eq!(
            writes(&p),
            vec![
                ("Cargo.lock".into(), Action::Write),
                ("Cargo.toml".into(), Action::Write),
                ("NOTES".into(), Action::TextMerge { meta_from: Side::Target, union: true }),
                ("mine".into(), Action::Write),
            ]
        );
        set(&mut x.t, &[("Cargo.lock", at(f(b"l2"), 20))]);
        set(&mut x.s, &[("a.bin", at(f(b"\0s"), 8))]);
        let p = go(&mut x, Run { src, dst, idents: (true, true), res: BTreeMap::new(), rules: &rules });
        assert_eq!(writes(&p).iter().map(|(p, _)| p.as_str()).collect::<Vec<_>>(), vec!["NOTES", "a.bin", "mine"]);
        let mut y = Fake::default();
        set(&mut y.o, &[("x.bin", f(b"\0o"))]);
        set(&mut y.t, &[("x.bin", at(f(b"\0t"), 1))]);
        let (rules, _) = Rules::parse(None, Some(b"*.bin newer\n"));
        let p = go(&mut y, Run { src: ch(&[("x.bin", Delete)]), dst: ch(&[("x.bin", Modify)]), idents: (true, true), res: BTreeMap::new(), rules: &rules });
        assert!(p.conflicts.is_empty() && p.deletes.is_empty(), "T's edit is newer than the deletion (at the root's time 0)");
    }

    #[test]
    fn many_writes() {
        let mut x = Fake::default();
        let mut src = Kinds::new();
        for i in 0..400 {
            let dir = format!("src/m{i}");
            for side in [&mut x.o, &mut x.s, &mut x.t] {
                set(side, &[("src", d()), (&dir, d())]);
            }
            for j in 0..100 {
                let p = format!("{dir}/f{j}.rs");
                set(&mut x.o, &[(&p, f(b"old"))]);
                set(&mut x.t, &[(&p, f(b"old"))]);
                set(&mut x.s, &[(&p, f(b"new"))]);
                src.insert(s(&p), Modify);
            }
        }
        let t = std::time::Instant::now();
        let p = run(&mut x, src, Kinds::new());
        assert_eq!((p.writes.len(), p.conflicts.len()), (40_000, 0));
        assert!(t.elapsed().as_secs() < 3);
    }
}
