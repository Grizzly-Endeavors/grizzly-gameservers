//! Wire types shared between the in-pod supervisor's HTTP control server and the
//! Discord bot's client. Kept dependency-light (serde only) so both sides agree
//! on the contract without pulling each other's transport stacks.
