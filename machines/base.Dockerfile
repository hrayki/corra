FROM ubuntu:24.04
ENV DEBIAN_FRONTEND=noninteractive LANG=C.UTF-8 IS_SANDBOX=1 DISABLE_AUTOUPDATER=1 PATH=/root/.local/bin:/root/.local/share/mise/shims:/usr/local/bin:/usr/bin:/bin
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl git git-lfs build-essential pkg-config libssl-dev unzip xz-utils zip jq ripgrep fd-find less procps psmisc python3 python3-venv openssh-client && rm -rf /var/lib/apt/lists/*
RUN curl -fsSL https://mise.run | MISE_INSTALL_PATH=/usr/local/bin/mise sh
RUN curl -fsSL https://deb.nodesource.com/setup_lts.x | bash - && apt-get install -y --no-install-recommends nodejs && rm -rf /var/lib/apt/lists/*
ENV PLAYWRIGHT_BROWSERS_PATH=/opt/ms-playwright
RUN npx -y playwright@latest install --with-deps chromium && npm install -g @playwright/mcp@latest && rm -rf /root/.npm /var/lib/apt/lists/*
RUN curl -fsSL https://claude.ai/install.sh | bash && cp -L /root/.local/bin/claude /usr/local/bin/claude && rm -rf /root/.local/bin/claude /root/.local/share/claude && /usr/local/bin/claude --version
RUN git config --system user.name "corra agent" && git config --system user.email "agent@corra.invalid" && git config --system init.defaultBranch main && printf '.playwright-mcp/\n' > /etc/gitignore && git config --system core.excludesFile /etc/gitignore
RUN mkdir -p /root/projects
WORKDIR /root
