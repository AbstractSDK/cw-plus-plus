#![doc = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/README.md"))]

use std::fmt::Display;

use abstract_std::{objects::gov_type::GovernanceDetails, AbstractError};
use cosmwasm_schema::cw_serde;
use cosmwasm_std::{Addr, Api, Attribute, BlockInfo, DepsMut, StdError, StdResult, Storage};
use cw_address_like::AddressLike;
// re-export the proc macros and the Expiration class
pub use cw_ownable_derive::{cw_ownable_execute, cw_ownable_query};
use cw_storage_plus::Item;
pub use cw_utils::Expiration;

/// The contract's ownership info
#[cw_serde]
pub struct Ownership<T: AddressLike> {
    /// The contract's current owner.
    pub owner: GovernanceDetails<T>,

    /// The account who has been proposed to take over the ownership.
    /// `None` if there isn't a pending ownership transfer.
    pub pending_owner: Option<GovernanceDetails<T>>,

    /// The deadline for the pending owner to accept the ownership.
    /// `None` if there isn't a pending ownership transfer, or if a transfer
    /// exists and it doesn't have a deadline.
    pub pending_expiry: Option<Expiration>,
}

/// Actions that can be taken to alter the contract's ownership
#[cw_serde]
pub enum Action {
    /// Propose to transfer the contract's ownership to another account,
    /// optionally with an expiry time.
    ///
    /// Can only be called by the contract's current owner.
    ///
    /// Any existing pending ownership transfer is overwritten.
    TransferOwnership {
        new_owner: GovernanceDetails<String>,
        expiry: Option<Expiration>,
    },

    /// Accept the pending ownership transfer.
    ///
    /// Can only be called by the pending owner.
    AcceptOwnership,

    /// Give up the contract's ownership and the possibility of appointing
    /// a new owner.
    ///
    /// Can only be invoked by the contract's current owner.
    ///
    /// Any existing pending ownership transfer is canceled.
    RenounceOwnership,
}

/// Errors associated with the contract's ownership
#[derive(thiserror::Error, Debug, PartialEq)]
pub enum OwnershipError {
    #[error("{0}")]
    Std(#[from] StdError),

    #[error("{0}")]
    Abstract(#[from] AbstractError),

    #[error("Contract ownership has been renounced")]
    NoOwner,

    #[error("Caller is not the contract's current owner")]
    NotOwner,

    #[error("Caller is not the contract's pending owner")]
    NotPendingOwner,

    #[error("There isn't a pending ownership transfer")]
    TransferNotFound,

    #[error("A pending ownership transfer exists but it has expired")]
    TransferExpired,
}

/// Storage constant for the contract's ownership
const OWNERSHIP: Item<Ownership<Addr>> = Item::new("ownership");

/// Set the given address as the contract owner.
///
/// This function is only intended to be used only during contract instantiation.
pub fn initialize_owner(
    deps: DepsMut,
    owner: GovernanceDetails<String>,
    version_control: Addr,
) -> Result<Ownership<Addr>, OwnershipError> {
    let ownership = Ownership {
        owner: owner.verify(deps.as_ref(), version_control)?,
        pending_owner: None,
        pending_expiry: None,
    };
    OWNERSHIP.save(deps.storage, &ownership)?;
    Ok(ownership)
}

/// Return Ok(true) if the contract has an owner and it's the given address.
/// Return Ok(false) if the contract doesn't have an owner, of if it does but
/// it's not the given address.
/// Return Err if fails to load ownership info from storage.
pub fn is_owner(store: &dyn Storage, addr: &Addr) -> StdResult<bool> {
    let ownership = OWNERSHIP.load(store)?;

    if let Some(owner) = ownership.owner.owner_address() {
        if *addr == owner {
            return Ok(true);
        }
    }

    Ok(false)
}

/// Assert that an account is the contract's current owner.
pub fn assert_owner(store: &dyn Storage, sender: &Addr) -> Result<(), OwnershipError> {
    let ownership = OWNERSHIP.load(store)?;
    check_owner(&ownership, sender)
}

/// Assert that an account is the contract's current owner.
fn check_owner(ownership: &Ownership<Addr>, sender: &Addr) -> Result<(), OwnershipError> {
    // the contract must have an owner
    let Some(current_owner) = &ownership.owner.owner_address() else {
        return Err(OwnershipError::NoOwner);
    };

    // the sender must be the current owner
    if sender != current_owner {
        return Err(OwnershipError::NotOwner);
    }

    Ok(())
}

/// Update the contract's ownership info based on the given action.
/// Return the updated ownership.
pub fn update_ownership(
    deps: DepsMut,
    block: &BlockInfo,
    sender: &Addr,
    version_control: Addr,
    action: Action,
) -> Result<Ownership<Addr>, OwnershipError> {
    match action {
        Action::TransferOwnership { new_owner, expiry } => {
            transfer_ownership(deps, sender, new_owner, version_control, expiry)
        },
        Action::AcceptOwnership => accept_ownership(deps.storage, block, sender),
        Action::RenounceOwnership => renounce_ownership(deps.storage, sender),
    }
}

/// Get the current ownership value.
pub fn get_ownership(storage: &dyn Storage) -> StdResult<Ownership<Addr>> {
    OWNERSHIP.load(storage)
}

impl<T: AddressLike> Ownership<T> {
    /// Serializes the current ownership state as attributes which may
    /// be used in a message response. Serialization is done according
    /// to the std::fmt::Display implementation for `T` and
    /// `cosmwasm_std::Expiration` (for `pending_expiry`). If an
    /// ownership field has no value, `"none"` will be serialized.
    ///
    /// Attribute keys used:
    ///  - owner
    ///  - pending_owner
    ///  - pending_expiry
    ///
    /// Callers should take care not to use these keys elsewhere
    /// in their response as CosmWasm will override reused attribute
    /// keys.
    ///
    /// # Example
    ///
    /// ```rust
    /// use cw_utils::Expiration;
    ///
    /// assert_eq!(
    ///     Ownership {
    ///         owner: Some("blue"),
    ///         pending_owner: None,
    ///         pending_expiry: Some(Expiration::Never {})
    ///     }
    ///     .into_attributes(),
    ///     vec![
    ///         Attribute::new("owner", "blue"),
    ///         Attribute::new("pending_owner", "none"),
    ///         Attribute::new("pending_expiry", "expiration: never")
    ///     ],
    /// )
    /// ```
    pub fn into_attributes(self) -> Vec<Attribute> {
        vec![
            Attribute::new("owner", self.owner.to_string()),
            Attribute::new("pending_owner", none_or(self.pending_owner.as_ref())),
            Attribute::new("pending_expiry", none_or(self.pending_expiry.as_ref())),
        ]
    }
}

fn none_or<T: Display>(or: Option<&T>) -> String {
    or.map_or_else(|| "none".to_string(), |or| or.to_string())
}

/// Propose to transfer the contract's ownership to the given address, with an
/// optional deadline.
fn transfer_ownership(
    deps: DepsMut,
    sender: &Addr,
    new_owner: GovernanceDetails<String>,
    version_control: Addr,
    expiry: Option<Expiration>,
) -> Result<Ownership<Addr>, OwnershipError> {
    let new_owner = new_owner.verify(deps.as_ref(), version_control)?;
    OWNERSHIP.update(deps.storage, |ownership| {
        // the contract must have an owner
        check_owner(&ownership, sender)?;

        // NOTE: We don't validate the expiry, i.e. asserting it is later than
        // the current block time.
        //
        // This is because if the owner submits an invalid expiry, it won't have
        // any negative effect - it's just that the pending owner won't be able
        // to accept the ownership.
        //
        // By not doing the check, we save a little bit of gas.
        //
        // To fix the erorr, the owner can simply invoke `transfer_ownership`
        // again with the correct expiry and overwrite the invalid one.
        Ok(Ownership {
            pending_owner: Some(new_owner),
            pending_expiry: expiry,
            ..ownership
        })
    })
}

/// Accept a pending ownership transfer.
fn accept_ownership(
    store: &mut dyn Storage,
    block: &BlockInfo,
    sender: &Addr,
) -> Result<Ownership<Addr>, OwnershipError> {
    OWNERSHIP.update(store, |ownership| {
        // there must be an existing ownership transfer
        let Some(maybe_pending_owner) = ownership.pending_owner else {
            return Err(OwnershipError::TransferNotFound);
        };

        // If new gov has an owner they must accept
        if let Some(pending_owner) = maybe_pending_owner.owner_address() {
            // the sender must be the pending owner
            if sender != pending_owner {
                return Err(OwnershipError::NotPendingOwner);
            };

            // if the transfer has a deadline, it must not have been reached
            if let Some(expiry) = &ownership.pending_expiry {
                if expiry.is_expired(block) {
                    return Err(OwnershipError::TransferExpired);
                }
            }
        };

        Ok(Ownership {
            owner: maybe_pending_owner,
            pending_owner: None,
            pending_expiry: None,
        })
    })
}

/// Set the contract's ownership as vacant permanently.
fn renounce_ownership(
    store: &mut dyn Storage,
    sender: &Addr,
) -> Result<Ownership<Addr>, OwnershipError> {
    OWNERSHIP.update(store, |ownership| {
        check_owner(&ownership, sender)?;

        Ok(Ownership {
            owner: GovernanceDetails::Renounced {},
            pending_owner: None,
            pending_expiry: None,
        })
    })
}

//------------------------------------------------------------------------------
// Tests
//------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use cosmwasm_std::{testing::mock_dependencies, Timestamp};

    use super::*;

    fn mock_addresses() -> [Addr; 3] {
        [
            Addr::unchecked("larry"),
            Addr::unchecked("jake"),
            Addr::unchecked("pumpkin"),
        ]
    }

    fn mock_govs() -> [GovernanceDetails<Addr>; 3] {
        [
            GovernanceDetails::Monarchy {
                monarch: Addr::unchecked("larry"),
            },
            GovernanceDetails::Monarchy {
                monarch: Addr::unchecked("jake"),
            },
            GovernanceDetails::Monarchy {
                monarch: Addr::unchecked("pumpkin"),
            },
        ]
    }

    fn vc_addr() -> Addr {
        Addr::unchecked("version_control")
    }

    fn mock_block_at_height(height: u64) -> BlockInfo {
        BlockInfo {
            height,
            time: Timestamp::from_seconds(10000),
            chain_id: "".into(),
        }
    }

    #[test]
    fn initializing_ownership() {
        let mut deps = mock_dependencies();
        let [larry, _, _] = mock_govs();

        let ownership = initialize_owner(deps.as_mut(), larry.clone().into(), vc_addr()).unwrap();

        // ownership returned is same as ownership stored.
        assert_eq!(ownership, OWNERSHIP.load(deps.as_ref().storage).unwrap());

        assert_eq!(
            ownership,
            Ownership {
                owner: larry,
                pending_owner: None,
                pending_expiry: None,
            },
        );
    }

    #[test]
    fn initialize_ownership_no_owner() {
        let mut deps = mock_dependencies();
        let ownership =
            initialize_owner(deps.as_mut(), GovernanceDetails::Renounced {}, vc_addr()).unwrap();
        assert_eq!(
            ownership,
            Ownership {
                owner: GovernanceDetails::Renounced {},
                pending_owner: None,
                pending_expiry: None,
            },
        );
    }

    #[test]
    fn asserting_ownership() {
        let mut deps = mock_dependencies();
        let [larry, jake, _] = mock_govs();

        // case 1. owner has not renounced
        {
            initialize_owner(deps.as_mut(), larry.clone().into(), vc_addr()).unwrap();

            let res = assert_owner(deps.as_ref().storage, &larry.owner_address().unwrap());
            assert!(res.is_ok());

            let res = assert_owner(deps.as_ref().storage, &jake.owner_address().unwrap());
            assert_eq!(res.unwrap_err(), OwnershipError::NotOwner);
        }

        // case 2. owner has renounced
        {
            renounce_ownership(deps.as_mut().storage, &larry.owner_address().unwrap()).unwrap();

            let res = assert_owner(deps.as_ref().storage, &larry.owner_address().unwrap());
            assert_eq!(res.unwrap_err(), OwnershipError::NoOwner);
        }
    }

    #[test]
    fn transferring_ownership() {
        let mut deps = mock_dependencies();
        let [larry, jake, pumpkin] = mock_govs();

        initialize_owner(deps.as_mut(), larry.clone().into(), vc_addr()).unwrap();

        // non-owner cannot transfer ownership
        {
            let err = update_ownership(
                deps.as_mut(),
                &mock_block_at_height(12345),
                &jake.owner_address().unwrap(),
                vc_addr(),
                Action::TransferOwnership {
                    new_owner: pumpkin.clone().into(),
                    expiry: None,
                },
            )
            .unwrap_err();
            assert_eq!(err, OwnershipError::NotOwner);
        }

        // owner properly transfers ownership
        {
            let ownership = update_ownership(
                deps.as_mut(),
                &mock_block_at_height(12345),
                &larry.owner_address().unwrap(),
                vc_addr(),
                Action::TransferOwnership {
                    new_owner: pumpkin.clone().into(),
                    expiry: Some(Expiration::AtHeight(42069)),
                },
            )
            .unwrap();
            assert_eq!(
                ownership,
                Ownership {
                    owner: larry,
                    pending_owner: Some(pumpkin),
                    pending_expiry: Some(Expiration::AtHeight(42069)),
                },
            );

            let saved_ownership = OWNERSHIP.load(deps.as_ref().storage).unwrap();
            assert_eq!(saved_ownership, ownership);
        }
    }

    #[test]
    fn accepting_ownership() {
        let mut deps = mock_dependencies();
        let [larry, jake, pumpkin] = mock_govs();

        initialize_owner(deps.as_mut(), larry.clone().into(), vc_addr()).unwrap();

        // cannot accept ownership when there isn't a pending ownership transfer
        {
            let err = update_ownership(
                deps.as_mut(),
                &mock_block_at_height(12345),
                &pumpkin.owner_address().unwrap(),
                vc_addr(),
                Action::AcceptOwnership,
            )
            .unwrap_err();
            assert_eq!(err, OwnershipError::TransferNotFound);
        }

        transfer_ownership(
            deps.as_mut(),
            &larry.owner_address().unwrap(),
            pumpkin.clone().into(),
            vc_addr(),
            Some(Expiration::AtHeight(42069)),
        )
        .unwrap();

        // non-pending owner cannot accept ownership
        {
            let err = update_ownership(
                deps.as_mut(),
                &mock_block_at_height(12345),
                &jake.owner_address().unwrap(),
                vc_addr(),
                Action::AcceptOwnership,
            )
            .unwrap_err();
            assert_eq!(err, OwnershipError::NotPendingOwner);
        }

        // cannot accept ownership if deadline has passed
        {
            let err = update_ownership(
                deps.as_mut(),
                &mock_block_at_height(69420),
                &pumpkin.owner_address().unwrap(),
                vc_addr(),
                Action::AcceptOwnership,
            )
            .unwrap_err();
            assert_eq!(err, OwnershipError::TransferExpired);
        }

        // pending owner properly accepts ownership before deadline
        {
            let ownership = update_ownership(
                deps.as_mut(),
                &mock_block_at_height(10000),
                &pumpkin.owner_address().unwrap(),
                vc_addr(),
                Action::AcceptOwnership,
            )
            .unwrap();
            assert_eq!(
                ownership,
                Ownership {
                    owner: pumpkin,
                    pending_owner: None,
                    pending_expiry: None,
                },
            );

            let saved_ownership = OWNERSHIP.load(deps.as_ref().storage).unwrap();
            assert_eq!(saved_ownership, ownership);
        }
    }

    #[test]
    fn renouncing_ownership() {
        let mut deps = mock_dependencies();
        let [larry, jake, pumpkin] = mock_govs();

        let ownership = Ownership {
            owner: larry.clone(),
            pending_owner: Some(pumpkin),
            pending_expiry: None,
        };
        OWNERSHIP.save(deps.as_mut().storage, &ownership).unwrap();

        // non-owner cannot renounce
        {
            let err = update_ownership(
                deps.as_mut(),
                &mock_block_at_height(12345),
                &jake.owner_address().unwrap(),
                vc_addr(),
                Action::RenounceOwnership,
            )
            .unwrap_err();
            assert_eq!(err, OwnershipError::NotOwner);
        }

        // owner properly renounces
        {
            let ownership = update_ownership(
                deps.as_mut(),
                &mock_block_at_height(12345),
                &larry.owner_address().unwrap(),
                vc_addr(),
                Action::RenounceOwnership,
            )
            .unwrap();

            // ownership returned is same as ownership stored.
            assert_eq!(ownership, OWNERSHIP.load(deps.as_ref().storage).unwrap());

            assert_eq!(
                ownership,
                Ownership {
                    owner: GovernanceDetails::Renounced {},
                    pending_owner: None,
                    pending_expiry: None,
                },
            );
        }

        // cannot renounce twice
        {
            let err = update_ownership(
                deps.as_mut(),
                &mock_block_at_height(12345),
                &larry.owner_address().unwrap(),
                vc_addr(),
                Action::RenounceOwnership,
            )
            .unwrap_err();
            assert_eq!(err, OwnershipError::NoOwner);
        }
    }

    #[test]
    fn into_attributes_works() {
        use cw_utils::Expiration;
        assert_eq!(
            Ownership {
                owner: GovernanceDetails::Monarchy {
                    monarch: "batman".to_string()
                },
                pending_owner: None,
                pending_expiry: Some(Expiration::Never {})
            }
            .into_attributes(),
            vec![
                Attribute::new("owner", "renounced"),
                Attribute::new("pending_owner", "none"),
                Attribute::new("pending_expiry", "expiration: never")
            ],
        );
    }
}
