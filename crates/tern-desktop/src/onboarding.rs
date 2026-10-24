//! The first-run tour (v2 s01–s02): welcome with the Tern mark, pick a
//! provider, paste its key and test it live, pick the default model, pick
//! where searches go, pick a theme. Skippable at every step, shown once
//! (`Preferences::onboarded`), and offered again from Settings › About.
//!
//! GPUI-free so the steps are unit tested; `view::onboarding_view` draws
//! the modal and slides the pages.

/// One page of the tour.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Step {
    #[default]
    Welcome,
    Provider,
    Key,
    Model,
    Search,
    Theme,
}

impl Step {
    pub const ALL: [Self; 6] = [
        Self::Welcome,
        Self::Provider,
        Self::Key,
        Self::Model,
        Self::Search,
        Self::Theme,
    ];

    /// The step rail's label.
    pub fn label(self) -> &'static str {
        match self {
            Self::Welcome => "Welcome",
            Self::Provider => "Provider",
            Self::Key => "API key",
            Self::Model => "Model",
            Self::Search => "Search",
            Self::Theme => "Theme",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::Welcome => "Welcome to Tern",
            Self::Provider => "Connect a model provider",
            Self::Key => "Paste your API key",
            Self::Model => "Pick a default model",
            Self::Search => "Where should searches go?",
            Self::Theme => "Pick a look",
        }
    }
}

/// How far a key test has got.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum KeyTest {
    #[default]
    NotYet,
    Testing,
    /// Accepted and saved; the line to show ("Connected · 14 models").
    Passed(String),
    /// The provider's own error.
    Failed(String),
}

/// The tour in progress.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Onboarding {
    pub step: Step,
    /// The provider picked on the Provider page.
    pub provider: Option<&'static str>,
    /// Whether that provider needs a key (a local server does not).
    pub needs_key: bool,
    /// A key for it is already in the keychain.
    pub key_saved: bool,
    pub test: KeyTest,
    pub model: Option<String>,
    pub search: Option<&'static str>,
    /// +1 when the last move went forward, -1 back: the page slides that way.
    pub direction: i8,
}

/// What the tour says to do when it ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Every step was seen.
    Finished,
    /// Skipped; the choices made so far stay.
    Skipped,
}

impl Onboarding {
    pub fn new() -> Self {
        Self {
            direction: 1,
            ..Self::default()
        }
    }

    /// The steps this tour walks: the key page only when the provider
    /// needs a key Tern does not have yet.
    pub fn steps(&self) -> Vec<Step> {
        Step::ALL
            .into_iter()
            .filter(|s| *s != Step::Key || self.wants_key_page())
            .collect()
    }

    fn wants_key_page(&self) -> bool {
        self.provider.is_none()
            || (self.needs_key && !self.key_saved)
            || matches!(self.test, KeyTest::Passed(_))
    }

    /// The current step's place, from 0, among [`Self::steps`].
    pub fn index(&self) -> usize {
        self.steps()
            .iter()
            .position(|s| *s == self.step)
            .unwrap_or(0)
    }

    /// Whether a step is behind the current one (the rail shows a check).
    pub fn done(&self, s: Step) -> bool {
        let steps = self.steps();
        match (
            steps.iter().position(|x| *x == s),
            steps.iter().position(|x| *x == self.step),
        ) {
            (Some(a), Some(b)) => a < b,
            _ => false,
        }
    }

    /// Picks a provider. A different one forgets the key test and model.
    pub fn pick_provider(&mut self, id: &'static str, needs_key: bool, key_saved: bool) {
        if self.provider != Some(id) {
            self.test = KeyTest::NotYet;
            self.model = None;
        }
        self.provider = Some(id);
        self.needs_key = needs_key;
        self.key_saved = key_saved;
    }

    /// Whether Continue is allowed on this page.
    pub fn can_continue(&self) -> bool {
        match self.step {
            Step::Welcome | Step::Search | Step::Theme => true,
            Step::Provider => self.provider.is_some(),
            Step::Key => matches!(self.test, KeyTest::Passed(_)),
            // No model listed yet still moves on: the provider's first chat
            // model is used.
            Step::Model => true,
        }
    }

    /// The key test started, passed, or failed.
    pub fn set_test(&mut self, t: KeyTest) {
        if matches!(t, KeyTest::Passed(_)) {
            self.key_saved = true;
        }
        self.test = t;
    }

    /// Continue. `Some(Finished)` after the last page.
    pub fn next(&mut self) -> Option<Outcome> {
        if !self.can_continue() {
            return None;
        }
        let steps = self.steps();
        let i = self.index();
        self.direction = 1;
        match steps.get(i + 1) {
            Some(s) => {
                self.step = *s;
                None
            }
            None => Some(Outcome::Finished),
        }
    }

    /// Back one page; nothing on the first.
    pub fn back(&mut self) {
        let steps = self.steps();
        let i = self.index();
        if i > 0 {
            self.direction = -1;
            self.step = steps[i - 1];
        }
    }

    /// Skip the rest.
    pub fn skip(&self) -> Outcome {
        Outcome::Skipped
    }

    /// Continue's label: the last page says so.
    pub fn continue_label(&self) -> &'static str {
        if self.index() + 1 == self.steps().len() {
            "Start using Tern"
        } else if self.step == Step::Welcome {
            "Get started"
        } else {
            "Continue"
        }
    }
}

/// Whether the tour opens at launch: never finished or skipped, and no
/// provider key on this computer yet (someone who set Tern up before the
/// tour existed is not walked through it; Settings › About offers it).
pub fn shows_at_launch(onboarded: bool, any_key_saved: bool) -> bool {
    !onboarded && !any_key_saved
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cloud_provider_walks_every_step() {
        let mut o = Onboarding::new();
        assert_eq!(o.step, Step::Welcome);
        assert_eq!(o.continue_label(), "Get started");
        assert_eq!(o.next(), None);
        assert_eq!(o.step, Step::Provider);
        assert!(!o.can_continue(), "a provider first");
        assert_eq!(o.next(), None);
        assert_eq!(o.step, Step::Provider);
        o.pick_provider("anthropic", true, false);
        o.next();
        assert_eq!(o.step, Step::Key);
        assert!(!o.can_continue(), "the key must pass its test");
        o.set_test(KeyTest::Testing);
        assert!(!o.can_continue());
        o.set_test(KeyTest::Failed("401 invalid x-api-key".into()));
        assert!(!o.can_continue());
        o.set_test(KeyTest::Passed("Connected · 14 models".into()));
        assert!(o.can_continue());
        assert!(o.done(Step::Provider) && !o.done(Step::Key));
        o.next();
        assert_eq!(o.step, Step::Model);
        // The key page stays in the rail after it passed.
        assert_eq!(o.steps().len(), 6);
        o.model = Some("claude-sonnet-4-5".into());
        o.next();
        assert_eq!(o.step, Step::Search);
        o.search = Some("wikipedia");
        o.next();
        assert_eq!(o.step, Step::Theme);
        assert_eq!(o.continue_label(), "Start using Tern");
        assert_eq!(o.next(), Some(Outcome::Finished));
    }

    #[test]
    fn a_local_server_or_a_saved_key_skips_the_key_page() {
        let mut o = Onboarding::new();
        o.next();
        o.pick_provider("ollama", false, false);
        o.next();
        assert_eq!(o.step, Step::Model);
        assert!(!o.steps().contains(&Step::Key));
        o.back();
        assert_eq!(o.step, Step::Provider);
        assert_eq!(o.direction, -1);
        o.pick_provider("groq", true, true);
        o.next();
        assert_eq!(o.step, Step::Model);
    }

    #[test]
    fn changing_provider_forgets_the_test_and_back_stops_at_welcome() {
        let mut o = Onboarding::new();
        o.pick_provider("openai", true, false);
        o.set_test(KeyTest::Passed("ok".into()));
        o.model = Some("gpt-5".into());
        o.pick_provider("openai", true, true);
        assert_eq!(o.model.as_deref(), Some("gpt-5"), "same provider keeps it");
        o.pick_provider("mistral", true, false);
        assert_eq!(o.test, KeyTest::NotYet);
        assert_eq!(o.model, None);
        o.back();
        assert_eq!(o.step, Step::Welcome);
        assert_eq!(o.skip(), Outcome::Skipped);
    }

    #[test]
    fn it_shows_once_and_not_to_people_already_set_up() {
        assert!(shows_at_launch(false, false));
        assert!(!shows_at_launch(true, false));
        assert!(!shows_at_launch(false, true));
    }
}
