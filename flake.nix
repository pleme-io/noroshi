{
  description = "noroshi — signal-fire alerts for AI coding agents";

  inputs.substrate.url = "github:pleme-io/substrate";

  outputs = { substrate, ... }: substrate.rust.tool {
    src = ./.;
    module = {
      description = "noroshi — signal-fire alerts for AI coding agents";
    };
  };
}
