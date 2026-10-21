#!/bin/sh
# Registers the runner on its first start (the registration is kept in the
# runner-home volume), then runs it.
set -eu
cd /home/runner/actions-runner

if [ ! -f .runner ]; then
    : "${RUNNER_TOKEN:?the runner is not registered yet: start it once with RUNNER_TOKEN set to a registration token (Settings > Actions > Runners > New self-hosted runner)}"
    ./config.sh --unattended --replace \
        --url "$RUNNER_URL" \
        --token "$RUNNER_TOKEN" \
        --name "$RUNNER_NAME" \
        --labels "$RUNNER_LABELS" \
        --work /home/runner/_work
fi

exec ./run.sh
