//! ADR 0036's checked-in opcode coverage policy.
//!
//! The table decides execution policy, not whether an opcode can be encoded.
//! Anything absent defaults to `refused`, so a new state-changing opcode fails
//! loudly while its name and complete tree still survive extraction and packing.

const IMPLEMENTED: &[&str] = &[
    "AddresseeFinderCaster",
    "AddresseeFinderSelf",
    "AddresseeFinderSingleMob",
    "AddresseeFinderTarget",
    "AttachAbility",
    "BuffAttacher",
    "BuffDetacher",
    "CombatStateTrigger",
    "CreatureNoticedTrigger",
    "DestinationLocator",
    "DeviceDie",
    "DeviceImpactsDeferred",
    "DoorSwitch",
    "EffectLinearStatModifier",
    "EffectTrigger",
    "EquipTrigger",
    "FloatData",
    "FloatZero",
    "ForceAggro",
    "FullHealthCalcer",
    "GoThroughPath",
    "Guard",
    "HealthTrigger",
    "ImpactActivateAggro",
    "ImpactAddExperience",
    "ImpactAttachTrigger",
    "ImpactClearTarget",
    "ImpactClientData",
    "ImpactClientDataCoords",
    "ImpactCreaturesAround",
    "ImpactDestroyItem",
    "ImpactDeviceDisintergrate",
    "ImpactDeviceSetVisualState",
    "ImpactDevicesAround",
    "ImpactDisintegrate",
    "ImpactFindPermanentDevice",
    "ImpactFindSingleDevice",
    "ImpactFindSingleMob",
    "ImpactFindSpawnTable",
    "ImpactGiveItem",
    "ImpactGoToLocator",
    "ImpactIfCaster",
    "ImpactIfScriptZoneVariable",
    "ImpactIfTarget",
    "ImpactIncreaseQuestCount",
    "ImpactInstantiating",
    "ImpactInstantiatingSimple",
    "ImpactKill",
    "ImpactLearnUp",
    "ImpactMobChat",
    "ImpactMobMorph",
    "ImpactRemoveAllBuffsFromGroup",
    "ImpactRemoveBuff",
    "ImpactRenewQuestMarks",
    "ImpactResetCombatAdvantage",
    "ImpactScriptZoneSetDisabled",
    "ImpactScriptZoneVariableSummand",
    "ImpactSetTarget",
    "ImpactStopSpawn",
    "ImpactStopTalk",
    "ImpactSummon",
    "ImpactTeleportLoc",
    "ImpactTurnMob",
    "ImpactsDeferred",
    "ImpactsToInterlocutor",
    "LifeGuard",
    "LinearEffectScaler",
    "LinerMultiplierScaler",
    "MarkedImpact",
    "PhysicalRangedScaler",
    "PhysicalScaler",
    "PredicateAnd",
    "PredicateCharacterClass",
    "PredicateCharacterRace",
    "PredicateEquipped",
    "PredicateHasCombatAdvantage",
    "PredicateHasItem",
    "PredicateIsAvatar",
    "PredicateIsDead",
    "PredicateIsMob",
    "PredicateMobWorld",
    "PredicateNot",
    "PredicateOr",
    "PredicateQuestStatus",
    "PredicateRemote",
    "PredicateVariableValueGreatThan",
    "PredicateVariableValueLessThan",
    "ProbabilisticImpact",
    "ProbabilisticImpactBinary",
    "RandomImpact",
    "ResetSpawnTable",
    "ReturningImpact",
    "ReturningInstantiatingImpact",
    "ScaledPhysicalDamage",
    "ScaledPhysicalWeaponDamage",
    "ScalerAllInputDamage",
    "ScalerAllOutputDamage",
    "SpawnSingleDevice",
    "SpawnSingleMob",
    "SpawnTableObjects",
    "Switch",
    "TagMobForKill",
    "TriggerAgentInterlocutor",
    "TriggerAgentOnTagged",
    "TriggerAgentSelf",
    "TriggerAgentSimple",
    "TriggerResource",
    "TrivialScaler",
    "WeaponSpeedScaler",
];

const INERT: &[&str] = &[
    "AutoAttackDisabler",
    "EffectDisableEvadeTimeout",
    "EffectDisableMove",
    "EffectDisableRotate",
    "EffectNoAggro",
    "EntityImpactCreaturesAround",
    "EntityImpactsOverTime",
    "ImpactLaunchProjectile",
    "ImpactsOverTime",
    "PredicateProjectile",
    "ScaledMagicDamage",
];

pub(crate) fn tier_for(opcode: &str) -> &'static str {
    if IMPLEMENTED.binary_search(&opcode).is_ok() {
        "implemented"
    } else if INERT.binary_search(&opcode).is_ok() {
        "inert-and-counted"
    } else {
        "refused"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_stay_sorted_and_disjoint() {
        assert!(IMPLEMENTED.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(INERT.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(
            IMPLEMENTED
                .iter()
                .all(|opcode| INERT.binary_search(opcode).is_err())
        );
    }

    #[test]
    fn unknown_opcodes_default_to_refused_without_an_encode_allow_list() {
        assert_eq!(tier_for("ImpactsDeferred"), "implemented");
        assert_eq!(tier_for("ImpactsOverTime"), "inert-and-counted");
        assert_eq!(tier_for("FutureStateChangingOpcode"), "refused");
    }
}
