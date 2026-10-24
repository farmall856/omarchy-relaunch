-- omarchy-relaunch root anchor. Installed root-owned 0644 at
-- /usr/share/omarchy-relaunch/anchor.lua; hyprland.lua runs it with
--   pcall(dofile, "/usr/share/omarchy-relaunch/anchor.lua")
--
-- It runs the root-owned verifier and evaluates only what that printed: the
-- boot registration of a signed config that verified, or a lone comment. The
-- text is loaded as text (never bytecode) in a fresh environment with no
-- metatable that holds o.exec_on_start and nothing else, and that accepts
-- only the one fixed boot registration. Windows are placed by the boot itself,
-- so there are no window rules to pass through.
local ANCHOR = "/usr/lib/omarchy-relaunch/relaunch-anchor"
local BOOT = "/usr/lib/omarchy-relaunch/relaunch-anchor boot"

local real = o
if type(real) ~= "table" or type(real.exec_on_start) ~= "function" then
  return
end
local exec_on_start = real.exec_on_start

local pipe = io.popen(ANCHOR .. " emit-lua", "r")
if pipe == nil then
  return
end
local text = pipe:read("*a")
pipe:close()
if type(text) ~= "string" or text == "" then
  return
end

local env = {
  o = {
    exec_on_start = function(command)
      if command ~= BOOT then
        error("refused o.exec_on_start: not the boot registration", 2)
      end
      return exec_on_start(command)
    end,
  },
}
local chunk, err = load(text, "=relaunch", "t", env)
if chunk == nil then
  print("omarchy-relaunch: anchor output did not load: " .. tostring(err))
  return
end
local ok, run_err = pcall(chunk)
if not ok then
  print("omarchy-relaunch: anchor output failed: " .. tostring(run_err))
end
