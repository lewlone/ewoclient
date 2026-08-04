//! `AnvilScreen`'s name field and what it sends (M93n).
//!
//! The **text entry itself is not here and not anywhere**: Rewo reads
//! `PhysicalKey`/`KeyCode` only, so nothing types. What is here is everything
//! between a string and the wire — the filter, the length rule, the
//! change-gate, and the normalisation that makes typing an item's own name
//! mean *clear the name*. The gap is recorded at the end of this module.

/// `StringUtil.isAllowedChatCharacter` — `ch != 167 && ch >= 32 && ch != 127`.
///
/// **167 is `§`**, the legacy formatting char M52d's chat styling parses. It is
/// excluded so a rename cannot inject colour codes; the other two are the
/// control range and DEL.
pub fn is_allowed_chat_character(ch: char) -> bool {
    let c = ch as u32;
    c != 167 && c >= 32 && c != 127
}

/// `StringUtil.filterText(input, multiline = false)`.
///
/// Vanilla iterates `toCharArray()` — UTF-16 code units — where this iterates
/// scalar values. The two agree: a supplementary character's surrogates are
/// both `>= 32` and neither is 167 or 127, so they survive there exactly as
/// the whole scalar survives here.
pub fn filter_text(input: &str) -> String {
    input
        .chars()
        .filter(|c| is_allowed_chat_character(*c))
        .collect()
}

/// `AnvilMenu.MAX_NAME_LENGTH`.
pub const MAX_NAME_LENGTH: usize = 50;

/// `AnvilMenu.validateName` — filter, then reject anything past 50.
///
/// **The length is Java's `String.length()`, which counts UTF-16 code units,
/// not characters.** An emoji is 2 there and 1 to `chars().count()`, so a
/// char-count check accepts names the server rejects — and the rejection is
/// silent, because `setItemName` simply returns false while the client has
/// already drawn the text.
///
/// `None` is "too long", which is **not** the same as an empty result: an
/// empty name is a legal request to clear the custom name.
pub fn validate_name(name: &str) -> Option<String> {
    let filtered = filter_text(name);
    (filtered.encode_utf16().count() <= MAX_NAME_LENGTH).then_some(filtered)
}

/// What `on_name_changed` needs to know about the stack in slot 0.
#[derive(Debug, Clone, Copy)]
pub struct AnvilInput<'a> {
    /// `stack.has(DataComponents.CUSTOM_NAME)`.
    pub has_custom_name: bool,
    /// `stack.getHoverName().getString()` — the DISPLAYED name, so a
    /// custom-named stack's is the custom one.
    pub hover_name: &'a str,
}

/// The anvil screen's name state — `AnvilMenu.itemName`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AnvilName {
    sent: String,
}

impl AnvilName {
    /// `AnvilMenu.setItemName` — whether the name **changed**, which is what
    /// gates the packet.
    ///
    /// ```java
    /// String validatedName = validateName(name);
    /// if (validatedName != null && !validatedName.equals(this.itemName)) {
    ///    this.itemName = validatedName;
    ///    ...
    ///    return true;
    /// } else return false;
    /// ```
    ///
    /// A name that survives the filter unchanged sends nothing, and an
    /// over-long one sends nothing **and does not update the stored name** —
    /// the next keystroke is compared against the last accepted value, not
    /// against what is on screen.
    pub fn set_item_name(&mut self, name: &str) -> bool {
        match validate_name(name) {
            Some(v) if v != self.sent => {
                self.sent = v;
                true
            }
            _ => false,
        }
    }

    /// What the server was last told.
    pub fn sent(&self) -> &str {
        &self.sent
    }

    /// `AnvilScreen.onNameChanged` — the string to send, or `None` for none.
    ///
    /// ```java
    /// Slot slot = this.menu.getSlot(0);
    /// if (slot.hasItem()) {
    ///    String newName = name;
    ///    if (!slot.getItem().has(CUSTOM_NAME)
    ///        && newName.equals(slot.getItem().getHoverName().getString())) {
    ///       newName = "";
    ///    }
    ///    if (this.menu.setItemName(newName)) send(new ServerboundRenameItemPacket(newName));
    /// }
    /// ```
    ///
    /// # Typing an item's own name means CLEAR the name
    ///
    /// If the stack carries **no** `CUSTOM_NAME` and you type exactly its
    /// default display name, the request sent is the **empty string** —
    /// because there is nothing to set: the item already reads that way, and
    /// asking for it explicitly would cost a level and stamp a custom name
    /// that changes nothing visible.
    ///
    /// Both halves of the guard matter. Drop the `!has(CUSTOM_NAME)` and
    /// renaming an already-renamed item *back* to its default silently clears
    /// it instead of setting it. Drop the equality and every rename becomes a
    /// clear.
    pub fn on_name_changed(
        &mut self,
        typed: &str,
        slot0: Option<AnvilInput<'_>>,
    ) -> Option<String> {
        // `if (slot.hasItem())` — an empty input slot sends nothing at all.
        let input = slot0?;
        let name = if !input.has_custom_name && typed == input.hover_name {
            ""
        } else {
            typed
        };
        self.set_item_name(name).then(|| self.sent.clone())
    }
}

// -- The recorded gap -------------------------------------------------------
//
// `EditBox` itself is absent: character input, the caret, selection, focus,
// and the `setEditable(slot0.hasItem())` / `slotChanged` re-seed. Rewo's key
// handler reads `PhysicalKey`/`KeyCode` and never `KeyEvent.text`, so nothing
// can type — this is a subsystem Rewo has never had rather than a wiring
// oversight, and it is shared with the chat/command-input cluster the coverage
// doc lists as class C.
//
// M93i's warning about unwired models applies with a smaller blast radius
// here: what it got wrong was the *shape* of a call site — an input enum with
// four fall-through cases it had guessed. This interface is a `&str` and a
// slot, and vanilla has exactly one caller for it.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_filter_strips_the_section_sign_and_the_control_range() {
        assert_eq!(filter_text("hi"), "hi");
        // 167 is §, excluded so a rename cannot inject colour codes.
        assert_eq!(filter_text("a\u{00A7}cb"), "acb");
        assert_eq!(filter_text("a\u{0000}b\u{001F}c"), "abc");
        assert_eq!(filter_text("a\u{007F}b"), "ab");
        // 32 is a space and IS allowed — the bound is `>= 32`, not `> 32`.
        assert_eq!(filter_text("a b"), "a b");
        // Newlines go, because `multiline` is false for an anvil.
        assert_eq!(filter_text("a\nb"), "ab");
        // Anything above the control range survives, including non-Latin and
        // supplementary characters.
        assert_eq!(filter_text("é日\u{1F600}"), "é日\u{1F600}");
    }

    #[test]
    fn the_length_limit_counts_utf16_units_and_not_characters() {
        // THE trap. Java's `String.length()` is code units, so an emoji is 2.
        // 25 emoji are 50 units and legal; 26 are 52 and are not — while
        // `chars().count()` would call them 25 and 26 and accept both.
        let ok: String = std::iter::repeat('\u{1F600}').take(25).collect();
        assert_eq!(ok.chars().count(), 25);
        assert_eq!(ok.encode_utf16().count(), 50);
        assert!(validate_name(&ok).is_some());

        let too_long: String = std::iter::repeat('\u{1F600}').take(26).collect();
        assert_eq!(too_long.chars().count(), 26, "a char count would allow this");
        assert!(validate_name(&too_long).is_none());

        // And the plain ASCII boundary, so the rule is not emoji-specific.
        assert!(validate_name(&"a".repeat(50)).is_some());
        assert!(validate_name(&"a".repeat(51)).is_none());
    }

    #[test]
    fn too_long_is_not_the_same_as_empty() {
        // `None` is a rejection; `Some("")` is a legal request to clear the
        // name. Collapsing them makes an over-long name silently clear one.
        assert_eq!(validate_name(""), Some(String::new()));
        assert_eq!(
            validate_name("\u{00A7}"),
            Some(String::new()),
            "filtered to nothing"
        );
        assert_eq!(validate_name(&"a".repeat(51)), None);
    }

    #[test]
    fn an_unchanged_name_sends_nothing_and_a_rejected_one_does_not_advance() {
        let mut n = AnvilName::default();
        assert!(n.set_item_name("Sting"));
        assert_eq!(n.sent(), "Sting");
        assert!(!n.set_item_name("Sting"), "unchanged sends nothing");
        // A name that FILTERS to the same thing is also unchanged.
        assert!(!n.set_item_name("Sting\u{00A7}"));
        // An over-long name is rejected AND leaves the stored name alone, so
        // the next comparison is against the last accepted value.
        assert!(!n.set_item_name(&"a".repeat(51)));
        assert_eq!(n.sent(), "Sting");
    }

    #[test]
    fn typing_an_items_own_name_means_clear_the_name() {
        let mut n = AnvilName::default();
        let plain = AnvilInput {
            has_custom_name: false,
            hover_name: "Diamond Sword",
        };
        // Typing exactly the default name normalises to "" — and since the
        // stored name is already "", nothing is sent at all.
        assert_eq!(n.on_name_changed("Diamond Sword", Some(plain)), None);
        assert_eq!(n.sent(), "");
        // Anything else is an ordinary rename.
        assert_eq!(n.on_name_changed("Sting", Some(plain)), Some("Sting".into()));
        // Now typing the default name DOES send, because the stored name is
        // "Sting" and the request is "".
        assert_eq!(
            n.on_name_changed("Diamond Sword", Some(plain)),
            Some(String::new())
        );
    }

    #[test]
    fn an_already_named_stack_is_not_normalised() {
        // The `!has(CUSTOM_NAME)` half. Dropping it makes renaming a named
        // item back to its displayed name CLEAR it rather than set it.
        let mut n = AnvilName::default();
        let named = AnvilInput {
            has_custom_name: true,
            hover_name: "Sting",
        };
        assert_eq!(
            n.on_name_changed("Sting", Some(named)),
            Some("Sting".into()),
            "a custom-named stack keeps the literal request"
        );
    }

    #[test]
    fn an_empty_input_slot_sends_nothing() {
        let mut n = AnvilName::default();
        assert_eq!(n.on_name_changed("Sting", None), None);
        assert_eq!(n.sent(), "", "and does not advance the stored name");
    }
}
