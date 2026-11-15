# shell/src/chat/composer

The composer card at the bottom of a chat.

- `mod.rs`: `Composer` state (text box, attached files, thinking level) and the `Thinking` levels.
- `attachments.rs`: `AttachedFile`, building the message the model receives (small text files inlined,
  others named by path), and the text the transcript shows. No UI types, so it is unit tested.
- `view.rs`: the card: attachment chips (Kit `Attachment`), the borderless Kit `Textarea`, and a toolbar with
  attach (+), Thinking and model menus, voice (disabled for now), and the send button. While the agent
  works the send button becomes an accent spinner that stops the run.

Files are added with the + button or by dropping them on the card.

Not wired yet: the Thinking level is not sent to the provider, the model menu lists only the active model,
and voice input needs a speech engine.
