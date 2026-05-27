//! Declared roles: parser, predicate

pub(crate) fn normalize_resume_list_args<I, S>(args: I) -> Vec<String>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let args = args.into_iter().map(Into::into).collect::<Vec<String>>();
    if legacy_resume_list_args(&args) {
        normalized_resume_list_args(args)
    } else {
        args
    }
}

fn legacy_resume_list_args(args: &[String]) -> bool {
    args.len() >= 4
        && args.get(1).is_some_and(|arg| arg == "resume")
        && args.get(2).is_some_and(|arg| arg == "--list")
}

fn normalized_resume_list_args(args: Vec<String>) -> Vec<String> {
    let mut normalized = Vec::with_capacity(args.len() - 1);
    normalized.push(args[0].clone());
    normalized.push(super::formatter::resume_list_subcommand_name());
    normalized.push(args[3].clone());
    normalized.extend(args.into_iter().skip(4));
    normalized
}
