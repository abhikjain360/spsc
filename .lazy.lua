return {
	{
		"mrcjkb/rustaceanvim",
		config = function()
			vim.g.rustaceanvim = {
				server = {
					default_settings = {
						-- rust-analyzer language server configuration
						["rust-analyzer"] = {
							cargo = {
								allFeatures = false,
								noDefaultFeatures = true,
								features = { "futures" },
							},
							checkOnSave = {
								allFeatures = false,
								noDefaultFeatures = true,
								features = { "futures" },
							},
						},
					},
				},
			}
		end,
	},
}
