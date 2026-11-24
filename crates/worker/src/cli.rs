//! Strict worker command and batch configuration parsing.
use crate::{paid_parent, reversal_items, reversal_pages, staff_recovery};
use commerce_infrastructure::payment_attempts::reversal_item_supervision::inbox::Cursor;

#[derive(PartialEq, Eq)]
pub(super) enum Mode {
    Probe,
    Expire,
    Reconcile,
    InspectPaymentRecovery(String),
    InspectPaymentCustody(String),
    Callbacks,
    ReversalPages,
    ReversalItems,
    EnrollReversalItem(reversal_items::Enrollment),
    InspectReversalItem(String),
    ReviewReversalItem(String),
    ReviewCancellation(String),
    ReviewCancellationInbox(Option<String>),
    InspectCancellationPacking(String),
    InspectHandoverEvidence(String),
    ReviewLateStockInbox(Option<String>),
    InspectRefundProposal(String),
    StaffRecovery {
        action: staff_recovery::Action,
        target: String,
        path: String,
    },
    InspectPacking(String),
    RecordPacking {
        order: String,
        effect: String,
        path: String,
    },
    ReleasePacking {
        order: String,
        target: String,
        effect: String,
    },
    ReviewReversalInbox(Option<Cursor>),
    EnrollReversal(reversal_pages::Enrollment),
    InspectReversal(String),
    InspectPaidParent(paid_parent::Inspection),
    InspectPaidParentSet(String),
}

fn staff_review_read(command: &str, target: &str) -> Option<Mode> {
    if !commerce_domain::order_stock::identifier(target) {
        return None;
    }
    Some(match command {
        "--review-cancellation" => Mode::ReviewCancellation(target.to_owned()),
        "--review-cancellation-inbox" => Mode::ReviewCancellationInbox(Some(target.to_owned())),
        "--inspect-cancellation-packing" => Mode::InspectCancellationPacking(target.to_owned()),
        "--inspect-handover-evidence" => Mode::InspectHandoverEvidence(target.to_owned()),
        "--review-late-stock-inbox" => Mode::ReviewLateStockInbox(Some(target.to_owned())),
        "--inspect-refund-proposal" => Mode::InspectRefundProposal(target.to_owned()),
        _ => return None,
    })
}

fn parse_mode(args: &[String]) -> Result<Mode, std::io::Error> {
    match args {
        [] => Ok(Mode::Probe),
        [argument] if argument == "--expire-holds" => Ok(Mode::Expire),
        [argument] if argument == "--reconcile-payments" => Ok(Mode::Reconcile),
        [argument, attempt]
            if argument == "--inspect-payment-recovery" && reversal_pages::identifier(attempt) =>
        {
            Ok(Mode::InspectPaymentRecovery(attempt.clone()))
        }
        [argument, attempt]
            if argument == "--inspect-payment-custody" && reversal_pages::identifier(attempt) =>
        {
            Ok(Mode::InspectPaymentCustody(attempt.clone()))
        }
        [argument] if argument == "--reconcile-callbacks" => Ok(Mode::Callbacks),
        [argument] if argument == "--reconcile-reversal-pages" => Ok(Mode::ReversalPages),
        [argument] if argument == "--reconcile-reversal-items" => Ok(Mode::ReversalItems),
        [argument, attempt, observation, page, ordinal, effect]
            if argument == "--enroll-reversal-item" =>
        {
            reversal_items::Enrollment::parse(attempt, observation, page, ordinal, effect)
                .map(Mode::EnrollReversalItem)
        }
        [argument, job]
            if argument == "--inspect-reversal-item" && reversal_pages::identifier(job) =>
        {
            Ok(Mode::InspectReversalItem(job.clone()))
        }
        [argument, job]
            if argument == "--review-reversal-item" && reversal_pages::identifier(job) =>
        {
            Ok(Mode::ReviewReversalItem(job.clone()))
        }
        [argument] if argument == "--review-cancellation-inbox" => {
            Ok(Mode::ReviewCancellationInbox(None))
        }
        [argument] if argument == "--review-late-stock-inbox" => {
            Ok(Mode::ReviewLateStockInbox(None))
        }
        [argument, order]
            if argument == "--inspect-packing"
                && commerce_domain::order_stock::identifier(order) =>
        {
            Ok(Mode::InspectPacking(order.clone()))
        }
        [argument, order, effect, path, confirmation]
            if argument == "--record-packing"
                && commerce_domain::order_stock::identifier(order)
                && commerce_domain::media::valid_command_id(effect)
                && !path.is_empty()
                && confirmation == "--confirm" =>
        {
            Ok(Mode::RecordPacking {
                order: order.clone(),
                effect: effect.clone(),
                path: path.clone(),
            })
        }
        [argument, order, target, effect, confirmation]
            if argument == "--release-packing"
                && commerce_domain::order_stock::identifier(order)
                && commerce_domain::order_stock::identifier(target)
                && commerce_domain::media::valid_command_id(effect)
                && confirmation == "--confirm" =>
        {
            Ok(Mode::ReleasePacking {
                order: order.clone(),
                target: target.clone(),
                effect: effect.clone(),
            })
        }
        [argument, target, path, confirmation]
            if confirmation == "--confirm"
                && !path.is_empty()
                && commerce_domain::order_stock::identifier(target) =>
        {
            staff_recovery::Action::from_flag(argument)
                .map(|action| Mode::StaffRecovery {
                    action,
                    target: target.clone(),
                    path: path.clone(),
                })
                .ok_or_else(|| std::io::Error::other("unsupported worker command"))
        }
        [argument] if argument == "--review-reversal-inbox" => Ok(Mode::ReviewReversalInbox(None)),
        [argument, timestamp, job] if argument == "--review-reversal-inbox" => {
            let time = timestamp
                .parse::<i64>()
                .ok()
                .filter(|time| time.to_string() == *timestamp)
                .ok_or_else(|| std::io::Error::other("unsupported worker command"))?;
            let cursor = Cursor::new(time, job)
                .map_err(|_| std::io::Error::other("unsupported worker command"))?;
            Ok(Mode::ReviewReversalInbox(Some(cursor)))
        }
        [argument, attempt, observation, kind, effect] if argument == "--enroll-reversal-scan" => {
            reversal_pages::Enrollment::parse(attempt, observation, kind, effect)
                .map(Mode::EnrollReversal)
        }
        [argument, job]
            if argument == "--inspect-reversal-scan" && reversal_pages::identifier(job) =>
        {
            Ok(Mode::InspectReversal(job.clone()))
        }
        [argument, attempt, effect] if argument == "--inspect-paid-parent" => {
            paid_parent::Inspection::parse(attempt, effect).map(Mode::InspectPaidParent)
        }
        [argument, attempt]
            if argument == "--inspect-paid-parent-set" && reversal_pages::identifier(attempt) =>
        {
            Ok(Mode::InspectPaidParentSet(attempt.clone()))
        }
        [argument, target] => staff_review_read(argument, target)
            .ok_or_else(|| std::io::Error::other("unsupported worker command")),
        _ => Err(std::io::Error::other("unsupported worker command")),
    }
}
pub(super) fn mode() -> Result<Mode, std::io::Error> {
    let args = std::env::args_os()
        .skip(1)
        .map(|arg| {
            arg.into_string()
                .map_err(|_| std::io::Error::other("unsupported worker command"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    parse_mode(&args)
}
pub(super) fn batch_limit() -> Result<u16, std::io::Error> {
    let raw = match std::env::var("HOLD_EXPIRY_BATCH") {
        Ok(value) => value,
        Err(std::env::VarError::NotPresent) => return Ok(25),
        Err(_) => return Err(std::io::Error::other("invalid HOLD_EXPIRY_BATCH")),
    };
    raw.parse::<u16>()
        .ok()
        .filter(|limit| (1..=100).contains(limit))
        .ok_or_else(|| std::io::Error::other("invalid HOLD_EXPIRY_BATCH"))
}

#[cfg(test)]
#[path = "../tests/support/command_modes.rs"]
mod command_modes_tests;
