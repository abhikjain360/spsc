return {
	{
		"neovim/nvim-lspconfig",
		opts = {
			servers = {
				rust_analyzer = {
					settings = {
						["rust-analyzer"] = {
							cargo = {
								allFeatures = true,
								-- "cfgs" tells the LSP to treat code guarded by
								-- #[cfg(loomer)] as active/enabled.
								cfgs = { "loomer" },
							},
							-- Optional: If 'cargo check' (diagnostics) fails because it
							-- doesn't know about the flag, you may also need this:
							check = {
								-- Passing the flag to the check command via RUSTFLAGS
								extraEnv = { RUSTFLAGS = "--cfg loomer" },
							},
						},
					},
				},
			},
		},
	},
}
