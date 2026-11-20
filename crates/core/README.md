# vivianne-core

Domain types shared by every crate: `ThreadId`, `Role`, `Message`, and `Mode` (Agent, Ask, Debug, Explore: what the agent is for, and which tools each
allows; a tool not named is withheld). No I/O, no async, only `serde`.
