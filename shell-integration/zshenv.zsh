# Restore the original startup directory while reading the user's .zshenv.
__vibeterm_startup_dir=$ZDOTDIR
if [[ $VIBETERM_ZDOTDIR_SET == 1 ]]; then
    ZDOTDIR=$VIBETERM_ZDOTDIR
else
    unset ZDOTDIR
fi
unset VIBETERM_ZDOTDIR VIBETERM_ZDOTDIR_SET
[[ -r ${ZDOTDIR-$HOME}/.zshenv ]] && source "${ZDOTDIR-$HOME}/.zshenv"
__vibeterm_user_zdotdir=${ZDOTDIR-}
__vibeterm_user_zdotdir_set=${+ZDOTDIR}
ZDOTDIR=$__vibeterm_startup_dir
unset __vibeterm_startup_dir
