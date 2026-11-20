//! Desktop notifications for when the window is not in front: something waiting on your approval, and a finished
//! reply.
//!
//! On Windows these are toasts, and a toast can carry buttons (Approve, Reject) or a reply box, so a question can be
//! answered without opening the window. Elsewhere nothing is shown (the in-app prompt is the only place to answer).
//! No GPUI types here: what the person chose is handed to a callback.

/// What the person picked on a notification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Choice {
    Approve,
    Reject,
    /// Typed into the notification's reply box.
    Reply(String),
    /// Clicked the notification itself.
    Open,
}

/// What a notification offers besides being read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quick {
    Nothing,
    ApproveReject,
    Reply,
}

/// The last thing the assistant said, short enough for a notification.
#[must_use]
pub fn last_reply(history: &[vivianne_core::Message]) -> String {
    history
        .iter()
        .rev()
        .find(|m| m.role == vivianne_core::Role::Assistant && !m.text.trim().is_empty())
        .map_or_else(|| "Task finished.".to_owned(), |m| m.text.trim().chars().take(120).collect())
}

/// Shows a notification under `tag` (so it can be taken down with [`withdraw`]). `on_choice` runs on a thread of
/// the system's choosing when the person answers.
pub fn show(tag: &str, title: &str, body: &str, quick: Quick, on_choice: impl Fn(Choice) + Send + Sync + 'static) {
    let body: String = body.chars().take(200).collect();
    #[cfg(windows)]
    {
        // A failed toast is not worth interrupting anything for.
        let _ = toast::show(tag, title, &body, quick, on_choice);
    }
    #[cfg(not(windows))]
    let _ = (tag, title, body, quick, on_choice);
}

/// Takes down a notification whose question was answered somewhere else.
pub fn withdraw(tag: &str) {
    #[cfg(windows)]
    toast::withdraw(tag);
    #[cfg(not(windows))]
    let _ = tag;
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&apos;")
}

#[cfg_attr(not(windows), allow(dead_code))]
fn xml(title: &str, body: &str, quick: Quick) -> String {
    let actions = match quick {
        Quick::Nothing => "",
        Quick::ApproveReject => r#"<action content="Approve" arguments="approve"/><action content="Reject" arguments="reject"/>"#,
        Quick::Reply => r#"<input id="reply" type="text" placeHolderContent="What next?"/><action content="Send" arguments="reply" hint-inputId="reply"/>"#,
    };
    format!(
        r#"<toast launch="open"><visual><binding template="ToastGeneric"><text>{}</text><text>{}</text></binding></visual><actions>{actions}</actions></toast>"#,
        escape(title),
        escape(body)
    )
}

#[cfg_attr(not(windows), allow(dead_code))]
fn choice(action: &str, reply: Option<String>) -> Choice {
    match action {
        "approve" => Choice::Approve,
        "reject" => Choice::Reject,
        "reply" => match reply.map(|r| r.trim().to_owned()).filter(|r| !r.is_empty()) {
            Some(r) => Choice::Reply(r),
            None => Choice::Open,
        },
        _ => Choice::Open,
    }
}

#[cfg(windows)]
mod toast {
    use std::sync::Mutex;

    use windows::Data::Xml::Dom::XmlDocument;
    use windows::Foundation::{IPropertyValue, TypedEventHandler};
    use windows::UI::Notifications::{ToastActivatedEventArgs, ToastNotification, ToastNotificationManager};
    use windows::core::{HSTRING, IInspectable, Interface};

    use super::{Choice, Quick, choice, xml};

    const GROUP: &str = "viv";
    /// An app with no installer has no identity of its own, and Windows drops toasts from an unknown one, so this
    /// borrows PowerShell's: the documented stand-in for unregistered apps.
    const APP_ID: &str = r"{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\WindowsPowerShell\v1.0\powershell.exe";

    /// Shown toasts, kept alive: a toast whose object is dropped stops delivering its button clicks. Capped, oldest out.
    static LIVE: Mutex<Vec<(String, ToastNotification)>> = Mutex::new(Vec::new());

    pub fn show(tag: &str, title: &str, body: &str, quick: Quick, on_choice: impl Fn(Choice) + Send + Sync + 'static) -> windows::core::Result<()> {
        let doc = XmlDocument::new()?;
        doc.LoadXml(&HSTRING::from(xml(title, body, quick)))?;
        let toast = ToastNotification::CreateToastNotification(&doc)?;
        toast.SetTag(&HSTRING::from(tag))?;
        toast.SetGroup(&HSTRING::from(GROUP))?;
        let owned_tag = tag.to_owned();
        toast.Activated(&TypedEventHandler::<ToastNotification, IInspectable>::new(move |_, args| {
            let Some(args) = args.as_ref().and_then(|a| a.cast::<ToastActivatedEventArgs>().ok()) else { return Ok(()) };
            let action = args.Arguments().map(|s| s.to_string()).unwrap_or_default();
            let reply = args
                .UserInput()
                .ok()
                .and_then(|set| set.Lookup(&HSTRING::from("reply")).ok())
                .and_then(|v| v.cast::<IPropertyValue>().ok())
                .and_then(|p| p.GetString().ok())
                .map(|s| s.to_string());
            on_choice(choice(&action, reply));
            forget(&owned_tag);
            Ok(())
        }))?;
        ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(APP_ID))?.Show(&toast)?;
        let mut live = LIVE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        live.push((tag.to_owned(), toast));
        if live.len() > 20 {
            live.remove(0);
        }
        Ok(())
    }

    fn forget(tag: &str) -> bool {
        let mut live = LIVE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        live.iter().position(|(t, _)| t == tag).map(|at| live.remove(at)).is_some()
    }

    /// Removes an unanswered toast from the screen and the Action Center.
    pub fn withdraw(tag: &str) {
        if forget(tag)
            && let Ok(history) = ToastNotificationManager::History()
        {
            let _ = history.RemoveGroupedTagWithId(&HSTRING::from(tag), &HSTRING::from(GROUP), &HSTRING::from(APP_ID));
        }
    }
}

#[cfg(test)]
mod tests {
    use vivianne_core::{Message, Role};

    use super::*;

    #[test]
    fn a_toast_escapes_its_text_and_offers_the_right_buttons() {
        let approve = xml("Needs <you>", "run \"rm -rf\" & more", Quick::ApproveReject);
        assert!(approve.contains("Needs &lt;you&gt;") && approve.contains("&quot;rm -rf&quot; &amp; more"));
        assert!(approve.contains(r#"arguments="approve""#) && approve.contains(r#"arguments="reject""#));
        assert!(xml("t", "b", Quick::Reply).contains(r#"id="reply""#));
        assert!(!xml("t", "b", Quick::Nothing).contains("<action "));
    }

    #[test]
    fn a_choice_is_what_was_pressed_and_an_empty_reply_is_just_a_click() {
        assert_eq!(choice("approve", None), Choice::Approve);
        assert_eq!(choice("reject", None), Choice::Reject);
        assert_eq!(choice("reply", Some("  go on ".into())), Choice::Reply("go on".into()));
        assert_eq!(choice("reply", Some("   ".into())), Choice::Open);
        assert_eq!(choice("open", None), Choice::Open);
    }

    #[test]
    fn the_notification_quotes_the_last_thing_the_assistant_said() {
        let history = vec![Message::new(Role::User, "hi"), Message::assistant("All done.", vec![]), Message::assistant("  ", vec![])];
        assert_eq!(last_reply(&history), "All done.");
        assert_eq!(last_reply(&[]), "Task finished.");
    }
}
