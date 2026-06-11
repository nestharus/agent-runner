pub(super) struct LifecycleInvocationRow {
    pub(super) invocation_uuid: String,
    pub(super) provider_name: Option<String>,
    pub(super) session_id: Option<String>,
    pub(super) provider_session_id: Option<String>,
    pub(super) resume_input_id: Option<String>,
}
