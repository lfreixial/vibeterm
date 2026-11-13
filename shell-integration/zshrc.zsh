if (( __vibeterm_user_zdotdir_set )); then
    ZDOTDIR=$__vibeterm_user_zdotdir
else
    unset ZDOTDIR
fi
unset __vibeterm_user_zdotdir __vibeterm_user_zdotdir_set
[[ -r ${ZDOTDIR-$HOME}/.zshrc ]] && source "${ZDOTDIR-$HOME}/.zshrc"
[[ -o interactive && -z ${__vibeterm_loaded-} ]] || return
__vibeterm_loaded=1
__vibeterm_running=0
# Zsh prints its missing-newline indicator BEFORE precmd. Finish the capture
# before that synthetic text; precmd remains the fallback when PROMPT_SP is off.
PROMPT_EOL_MARK=$'%{\e]2;__vibeterm_shell__;D\a%}'${PROMPT_EOL_MARK-'%B%S%#%s%b'}
__vibeterm_preexec() {
    __vibeterm_running=1
    printf '\e]2;__vibeterm_shell__;C\a' >&2
}
__vibeterm_precmd() {
    if (( __vibeterm_running )); then
        printf '\e]2;__vibeterm_shell__;D\a' >&2
    fi
    __vibeterm_running=0
    printf '\e]2;__vibeterm_shell__;A\a' >&2
}
precmd_functions=($precmd_functions __vibeterm_precmd)
preexec_functions=($preexec_functions __vibeterm_preexec)
