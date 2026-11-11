//! The logos-escrow program (github.com/drptbl/logos-escrow): a buyer locks a
//! job's payment and names where it is paid out. The provider pulls it after
//! delivering, and the buyer pulls it back when nothing is delivered by the
//! deadline or the provider returns the job. This module only encodes and
//! decodes; the server sends and reads through `lez_core`.

use logos_mcp::guard;
use sha2::{Digest, Sha256};

/// The program deployed on the LEZ testnet (v0.3.1), immutable: logos-escrow
/// with payout addresses and pull-only payouts (image 1d646179…).
pub const TESTNET_PROGRAM_ID: &str = "HpavNGFYxjyhLsy171aXsvv146yUeXmRWiho22uvT43d";

/// Names another deploy of the same program to use instead, such as a local
/// devnet's: its id, base58 or hex.
pub const PROGRAM_ENV: &str = "LOGOS_PROGRAM_ESCROW";

/// The escrow program this server pays into and reads.
pub struct Program {
    /// Base58, as quotes name it.
    pub id: String,
    /// Hex, as `lez_core` takes it.
    pub hex: String,
    bytes: Id,
}

impl Program {
    /// The program named by `configured`, or the testnet's when unset. Set
    /// but blank is refused: it is a mistake, not a choice of the testnet.
    pub fn parse(configured: Option<&str>) -> Result<Self, String> {
        let named = configured.map_or(TESTNET_PROGRAM_ID, str::trim);
        let bad = || format!("{PROGRAM_ENV} is not a program id, base58 or hex: {named}");
        let hex = guard::public_account_hex(named)
            .or_else(|_| guard::public_account_hex(&format!("Public/{named}")))
            .map_err(|_| bad())?;
        let id = guard::account_id_base58(&hex).ok_or_else(bad)?;
        let mut bytes = [0u8; 32];
        for (i, b) in bytes.iter_mut().enumerate() {
            *b = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).map_err(|_| bad())?;
        }
        Ok(Self { id, hex, bytes })
    }
}

static PROGRAM: std::sync::OnceLock<Program> = std::sync::OnceLock::new();

/// The program [`PROGRAM_ENV`] names.
fn from_env() -> Result<Program, String> {
    match std::env::var(PROGRAM_ENV) {
        Ok(named) => Program::parse(Some(&named)),
        Err(std::env::VarError::NotPresent) => Program::parse(None),
        Err(std::env::VarError::NotUnicode(_)) => Err(format!("{PROGRAM_ENV} is not UTF-8")),
    }
}

/// Read the configured program before serving, so a bad id stops the server
/// at once rather than at its first job.
pub fn check_env() -> Result<(), String> {
    if PROGRAM.get().is_none() {
        let _ = PROGRAM.set(from_env()?);
    }
    Ok(())
}

/// The program from [`PROGRAM_ENV`], or the testnet's. Read once.
pub fn program() -> &'static Program {
    PROGRAM.get_or_init(|| from_env().unwrap_or_else(|e| panic!("{e}")))
}

const JOB_SEED: &[u8; 32] = b"/lez-escrow/v1/JobSeed/000000000";
const PDA_PREFIX: &[u8; 32] = b"/LEE/v0.2/AccountId/PDA/\0\0\0\0\0\0\0\0";

pub type Id = [u8; 32];

/// The account a buyer's job lives in. Deriving from the buyer too means no
/// one who learns `job_ref` can open that job first.
pub fn job_account(buyer: &Id, job_ref: &Id) -> Id {
    job_account_in(&program().bytes, buyer, job_ref)
}

/// The job account under `program`.
fn job_account_in(program: &Id, buyer: &Id, job_ref: &Id) -> Id {
    let seed: Id = Sha256::new()
        .chain_update(JOB_SEED)
        .chain_update(buyer)
        .chain_update(job_ref)
        .finalize()
        .into();
    // The PDA prefix is 24 bytes of text and 8 zero bytes: 32 in all.
    Sha256::new()
        .chain_update(PDA_PREFIX)
        .chain_update(program)
        .chain_update(seed)
        .finalize()
        .into()
}

/// The program's instructions, Borsh-encoded: a variant byte, then the fields.
/// Only `Claim` and `Refund` move money, each to the address the job names.
#[derive(Debug, Clone, PartialEq)]
pub enum Instruction {
    Open {
        job_ref: Id,
        provider: Id,
        /// Where the provider is paid.
        pay_to: Id,
        /// Where the deposit goes back.
        refund_to: Id,
        amount: u128,
        deadline: u64,
    },
    Deliver {
        result: Id,
        deadline: u64,
    },
    Claim {
        buyer: Id,
        job_ref: Id,
        amount: u128,
    },
    /// The buyer lets the provider claim without delivering.
    Release {
        job_ref: Id,
    },
    Refund {
        job_ref: Id,
        amount: u128,
        /// The job's deadline, or 0 after a `Return`.
        deadline: u64,
    },
    /// The provider gives the job back, so the buyer can refund at once.
    Return,
}

impl Instruction {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(153);
        match self {
            Self::Open {
                job_ref,
                provider,
                pay_to,
                refund_to,
                amount,
                deadline,
            } => {
                out.push(0);
                out.extend(job_ref);
                out.extend(provider);
                out.extend(pay_to);
                out.extend(refund_to);
                out.extend(amount.to_le_bytes());
                out.extend(deadline.to_le_bytes());
            }
            Self::Deliver { result, deadline } => {
                out.push(1);
                out.extend(result);
                out.extend(deadline.to_le_bytes());
            }
            Self::Claim {
                buyer,
                job_ref,
                amount,
            } => {
                out.push(2);
                out.extend(buyer);
                out.extend(job_ref);
                out.extend(amount.to_le_bytes());
            }
            Self::Release { job_ref } => {
                out.push(3);
                out.extend(job_ref);
            }
            Self::Refund {
                job_ref,
                amount,
                deadline,
            } => {
                out.push(4);
                out.extend(job_ref);
                out.extend(amount.to_le_bytes());
                out.extend(deadline.to_le_bytes());
            }
            Self::Return => out.push(5),
        }
        out
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    Open,
    Delivered,
    Claimed,
    /// The buyer released it: the provider may claim without delivering.
    Released,
    Refunded,
    /// The provider gave it back: the buyer may refund at once.
    Returned,
}

/// A job as the program records it.
#[derive(Debug, Clone, PartialEq)]
pub struct Record {
    pub buyer: Id,
    pub provider: Id,
    pub pay_to: Id,
    pub refund_to: Id,
    pub amount: u128,
    /// Milliseconds since the Unix epoch.
    pub deadline: u64,
    pub result: Option<Id>,
    pub state: JobState,
}

impl Record {
    pub fn decode(raw: &[u8]) -> Option<Self> {
        let id = |at: usize| -> Option<Id> { raw.get(at..at + 32)?.try_into().ok() };
        let buyer = id(0)?;
        let provider = id(32)?;
        let pay_to = id(64)?;
        let refund_to = id(96)?;
        let amount = u128::from_le_bytes(raw.get(128..144)?.try_into().ok()?);
        let deadline = u64::from_le_bytes(raw.get(144..152)?.try_into().ok()?);
        let (result, at) = match raw.get(152)? {
            0 => (None, 153),
            1 => (Some(id(153)?), 185),
            _ => return None,
        };
        let state = match raw.get(at)? {
            0 => JobState::Open,
            1 => JobState::Delivered,
            2 => JobState::Claimed,
            3 => JobState::Released,
            4 => JobState::Refunded,
            5 => JobState::Returned,
            _ => return None,
        };
        (raw.len() == at + 1).then_some(Self {
            buyer,
            provider,
            pay_to,
            refund_to,
            amount,
            deadline,
            result,
            state,
        })
    }
}

/// The job record in an account, as `lez_core`'s `get_account_public` returns
/// it: `Ok(None)` when the account holds no job.
pub fn record_in(account: &str) -> Result<Option<Record>, String> {
    let v: serde_json::Value =
        serde_json::from_str(account).map_err(|e| format!("unreadable account: {e}"))?;
    let shard = v["shards"].as_array().and_then(|shards| {
        shards
            .iter()
            .find(|s| s["program"] == program().hex.as_str())
    });
    let Some(data) = shard.and_then(|s| s["data"].as_str()) else {
        return Ok(None);
    };
    let raw: Option<Vec<u8>> = (0..data.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(data.get(i..i + 2)?, 16).ok())
        .collect();
    raw.as_deref()
        .and_then(Record::decode)
        .map(Some)
        .ok_or_else(|| "the escrow record doesn't decode".to_string())
}

#[cfg(test)]
mod tests;
