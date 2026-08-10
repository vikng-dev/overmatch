//! The one finding shape every asset-door stage emits, and its deterministic rendering.
//!
//! Text, JSON and GUI output are renderings of this shape; the exit status is [`has_error`]. A
//! check's severity is compiled into its [`Check`] definition beside its id — there is no severity
//! file, allowlist, per-asset override or warning budget anywhere in the door.

use std::fmt;

/// Which pass a check belongs to. The report's primary sort key, declared in pipeline order.
///
/// `Door` precedes every pass: it carries the door's own mechanical refusals, which happen before a
/// check OF the asset can be reached — an input the door failed to supply is not a defect of the
/// model.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Stage {
    /// The wrapper's own mechanical refusals (`door.*`).
    Door,
    /// The `.blend` source pass (`L1.*`).
    Source,
    /// The shared consumer contract (`L2.*`) — this module's callers.
    Consumer,
    /// The GLB derivation: texture encode and repack (`D.*`).
    Derivation,
}

/// A finding's tier. Declared in report order, so a sort puts the build-failing rows first.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Severity {
    /// A real refusal: the asset does not enter the sim. Always build-failing.
    Error,
    /// Generic evidence needing human judgement. Never changes exit status.
    Warning,
    /// Census and change evidence only.
    Info,
}

impl Severity {
    fn label(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warning => "warning",
            Self::Info => "info",
        }
    }
}

/// What a finding is about.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum SubjectKind {
    /// The door itself; the subject name is the stage that refused.
    Door,
    /// The `.tank.ron` spec sheet; the subject name is its path.
    Spec,
    /// The GLB document as a whole; the subject name is its path.
    Document,
    /// One glTF node; the subject name is the node name.
    Node,
}

impl SubjectKind {
    fn label(self) -> &'static str {
        match self {
            Self::Door => "door",
            Self::Spec => "spec",
            Self::Document => "document",
            Self::Node => "node",
        }
    }
}

/// The thing a finding names: kind, name, and the element inside it (a primitive, a triangle, an
/// edge, a shell) where the check found something smaller than the whole subject.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct Subject {
    pub kind: SubjectKind,
    pub name: String,
    pub element: Option<String>,
}

impl Subject {
    pub(crate) fn door(stage: &str) -> Self {
        Self {
            kind: SubjectKind::Door,
            name: stage.to_owned(),
            element: None,
        }
    }

    pub(crate) fn spec(path: &std::path::Path) -> Self {
        Self {
            kind: SubjectKind::Spec,
            name: path.display().to_string(),
            element: None,
        }
    }

    pub(crate) fn document(path: &std::path::Path) -> Self {
        Self {
            kind: SubjectKind::Document,
            name: path.display().to_string(),
            element: None,
        }
    }

    pub(crate) fn node(name: &str) -> Self {
        Self {
            kind: SubjectKind::Node,
            name: name.to_owned(),
            element: None,
        }
    }

    pub(crate) fn element(mut self, element: impl Into<String>) -> Self {
        self.element = Some(element.into());
        self
    }
}

impl fmt::Display for Subject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} `{}`", self.kind.label(), self.name)?;
        match &self.element {
            Some(element) => write!(f, " {element}"),
            None => Ok(()),
        }
    }
}

/// One check: its id, the pass and tier compiled in beside it, and the law it enforces.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Check {
    pub id: &'static str,
    pub stage: Stage,
    pub severity: Severity,
    /// The law, stated as the condition that must hold.
    pub law: &'static str,
}

/// One violation: which check, of what, with what was measured and what repairs it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Finding {
    pub check: &'static Check,
    pub subject: Subject,
    /// What was measured — the numbers, names and coordinates the verdict rests on.
    pub evidence: String,
    /// The concrete edit that makes the law hold.
    pub repair: String,
}

impl Finding {
    pub(crate) fn new(
        check: &'static Check,
        subject: Subject,
        evidence: impl Into<String>,
        repair: impl Into<String>,
    ) -> Self {
        Self {
            check,
            subject,
            evidence: evidence.into(),
            repair: repair.into(),
        }
    }

    /// Stage, severity, check id, subject, element — the declared report order. Evidence breaks a
    /// remaining tie, so two findings that differ at all still order deterministically.
    fn sort_key(&self) -> (Stage, Severity, &'static str, &Subject, &str) {
        (
            self.check.stage,
            self.check.severity,
            self.check.id,
            &self.subject,
            self.evidence.as_str(),
        )
    }
}

impl fmt::Display for Finding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "{} {}: {}",
            self.check.id,
            self.check.severity.label(),
            self.subject
        )?;
        writeln!(f, "  measured: {}", self.evidence)?;
        writeln!(f, "  law:      {}", self.check.law)?;
        write!(f, "  repair:   {}", self.repair)
    }
}

/// Put a report in its declared order. Every producer sorts before it returns, so the console, a
/// panic message and a future JSON rendering read the same rows in the same order.
pub(crate) fn sorted(mut findings: Vec<Finding>) -> Vec<Finding> {
    findings.sort_by(|a, b| a.sort_key().cmp(&b.sort_key()));
    findings
}

/// Whether the report fails the build.
pub fn has_error(findings: &[Finding]) -> bool {
    findings
        .iter()
        .any(|finding| finding.check.severity == Severity::Error)
}

/// The complete text rendering — every finding, one block each.
pub fn render(findings: &[Finding]) -> String {
    findings
        .iter()
        .map(|finding| format!("{finding}\n"))
        .collect()
}
