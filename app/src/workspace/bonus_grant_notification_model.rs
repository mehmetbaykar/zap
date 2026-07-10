use warpui::{Entity, ModelContext, SingletonEntity};

/// Zap (Phase 3c A1): this model used to observe the cloud `AIRequestUsageModel` and toast a
/// notification whenever the server granted bonus/referral AI-request credits. That whole
/// subscription-quota subsystem (`AIRequestUsageModel`, `AIRequestUsageModelEvent`,
/// `BonusGrant`, `BonusGrantScope`) was removed, so there is no longer any grant source to
/// observe or notify about. The type is kept as an inert singleton because `lib.rs` still
/// registers it and `workspace/view.rs` still subscribes to its (now never-emitted) event.
#[derive(Copy, Clone, Debug)]
pub struct BonusGrantNotificationModel;

#[derive(Debug, Clone)]
pub enum BonusGrantNotificationEvent {
    ShowNotification { message: String },
}

impl Entity for BonusGrantNotificationModel {
    type Event = BonusGrantNotificationEvent;
}

impl SingletonEntity for BonusGrantNotificationModel {}

impl BonusGrantNotificationModel {
    pub fn new(_ctx: &mut ModelContext<Self>) -> Self {
        Self
    }
}
