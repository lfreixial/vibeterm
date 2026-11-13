# Loaded after the user's .bashrc. Do not replace an existing DEBUG trap (for
# example bash-preexec), or install into shells with inherited DEBUG tracing.
[[ $- == *i* && $- != *T* && -z ${__vibeterm_loaded-} ]] || return
[[ -z $(trap -p DEBUG) ]] || return
__vibeterm_loaded=1
__vibeterm_ready=0
__vibeterm_running=0
__vibeterm_preexec() {
    local status=$?
    if [[ $__vibeterm_ready == 1 && $BASH_COMMAND != __vibeterm_precmd ]]; then
        __vibeterm_ready=0
        __vibeterm_running=1
        printf '\e]2;__vibeterm_shell__;C\a' >&2
    fi
    return "$status"
}
__vibeterm_precmd() {
    local status=$?
    __vibeterm_ready=0
    if [[ $__vibeterm_running == 1 ]]; then
        printf '\e]2;__vibeterm_shell__;D\a' >&2
    fi
    __vibeterm_running=0
    printf '\e]2;__vibeterm_shell__;A\a' >&2
    return "$status"
}
__vibeterm_prompt_ready() { __vibeterm_ready=1; }
# Array assignment preserves both scalar and array PROMPT_COMMAND configurations.
PROMPT_COMMAND=(__vibeterm_precmd "${PROMPT_COMMAND[@]}" __vibeterm_prompt_ready)
trap '__vibeterm_preexec' DEBUG
