use crate::{InputRectangle, OverlayError, Result};

/// Meaning of the active text field, used as a hint to the input method.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TextInputPurpose {
    /// Ordinary text.
    #[default]
    Normal,
    /// Numeric text, including signs and decimal separators.
    Number,
    /// Telephone number.
    Phone,
    /// Email address.
    Email,
    /// Hidden password; surrounding text must not be supplied.
    Password,
    /// Hidden numeric PIN; surrounding text must not be supplied.
    Pin,
    /// Terminal-style input.
    Terminal,
}

/// Origin of an application update to surrounding text.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TextChangeCause {
    /// The application applied a commit or deletion from the input method.
    InputMethod,
    /// Cursor movement or another application change invalidated composition.
    #[default]
    Other,
}

/// UTF-8 document excerpt surrounding the insertion point.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SurroundingText {
    /// At most 4000 bytes, including the complete selection.
    pub text: String,
    /// Insertion point, as a UTF-8 byte boundary within `text`.
    pub cursor: usize,
    /// Other end of the selection, as a UTF-8 byte boundary within `text`.
    pub anchor: usize,
}

/// Application-owned text field state. Text input is disabled by default.
/// Raw and translated keyboard events remain available independently.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextInputState {
    /// Whether the focused overlay currently contains an editing field.
    pub enabled: bool,
    /// Optional UTF-8 context. X11 XIM does not support this context request.
    pub surrounding: Option<SurroundingText>,
    /// Logical caret rectangle used to position input-method UI.
    pub cursor_rectangle: InputRectangle,
    /// Field purpose hint; XIM servers may ignore this hint.
    pub purpose: TextInputPurpose,
    /// Why the application changed the document excerpt or cursor.
    pub change_cause: TextChangeCause,
}

impl Default for TextInputState {
    fn default() -> Self {
        Self {
            enabled: false,
            surrounding: None,
            cursor_rectangle: InputRectangle::new(0, 0, 1, 1),
            purpose: TextInputPurpose::Normal,
            change_cause: TextChangeCause::Other,
        }
    }
}

impl TextInputState {
    /// Enables an ordinary text field with no surrounding document excerpt.
    pub fn enabled() -> Self {
        Self {
            enabled: true,
            ..Self::default()
        }
    }
    pub(crate) fn validate(&self) -> Result<()> {
        self.cursor_rectangle.validate()?;
        if i32::from(self.cursor_rectangle.x) + i32::from(self.cursor_rectangle.width)
            > i32::from(i16::MAX)
            || i32::from(self.cursor_rectangle.y) + i32::from(self.cursor_rectangle.height)
                > i32::from(i16::MAX)
        {
            return Err(OverlayError::InvalidConfiguration(
                "text cursor rectangle exceeds portable input-method coordinates".into(),
            ));
        }
        if let Some(context) = &self.surrounding {
            if context.text.len() > 4000
                || !context.text.is_char_boundary(context.cursor)
                || !context.text.is_char_boundary(context.anchor)
                || context.text.contains('\0')
            {
                return Err(OverlayError::InvalidConfiguration("surrounding text must fit 4000 bytes and use valid UTF-8 cursor/anchor boundaries".into()));
            }
            if matches!(
                self.purpose,
                TextInputPurpose::Password | TextInputPurpose::Pin
            ) {
                return Err(OverlayError::InvalidConfiguration(
                    "hidden fields must not expose surrounding text".into(),
                ));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn document_context_uses_byte_boundaries_and_excludes_hidden_fields() {
        let mut state = TextInputState::enabled();
        state.surrounding = Some(SurroundingText {
            text: "é文".into(),
            cursor: 2,
            anchor: 5,
        });
        assert!(state.validate().is_ok());
        state.surrounding.as_mut().unwrap().cursor = 1;
        assert!(state.validate().is_err());
        state.surrounding.as_mut().unwrap().cursor = 2;
        state.purpose = TextInputPurpose::Password;
        assert!(state.validate().is_err());
    }
}
