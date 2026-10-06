-- Random live-ID workload: relative weights 100 feeds, 100 posts, 15 likes, 2 creates.
-- Every implementation receives this same script; writes are real and IDs come from feed responses.
local tokens, ids = {}, {500000}
local serial = 0
local threads = {}
errors = 0
function setup(thread) threads[#threads+1] = thread end
function init(args)
  local f = assert(io.open(os.getenv("TOKENS") or "/bench/seed/tokens.json", "r"))
  for token in f:read("*a"):gmatch('"token"%s*:%s*"([^"]+)"') do tokens[#tokens+1] = token end
  f:close(); assert(#tokens>0)
  math.randomseed(42)
end
function request()
  local n = math.random(217)
  if n<=100 then return wrk.format("GET", "/feed") end
  local id = ids[math.random(#ids)]
  if n<=200 then return wrk.format("GET", "/posts/"..id) end
  local headers = {Authorization="Bearer "..tokens[math.random(#tokens)],["Content-Type"]="application/json"}
  if n<=215 then return wrk.format("POST", "/posts/"..id.."/like",headers,"") end
  serial = serial+1
  return wrk.format("POST", "/posts",headers,
    '{"body":"local mixed workload post '..serial..': measuring fresh writes and reads"}')
end
function response(status, headers, body)
  if status~=200 and status~=201 then errors=errors+1 end
  if body:sub(1,10)=='{"posts":[' then
    local fresh={}; for id in body:gmatch('"id":(%d+)') do fresh[#fresh+1]=tonumber(id) end
    if #fresh==0 then errors=errors+1 else ids=fresh end
  end
end
function done(summary, latency, requests)
  local total=0; for _,t in ipairs(threads) do total=total+(t:get("errors") or 0) end
  io.write("HTTP semantic errors: "..total.."\n")
  io.write(string.format("LATENCY p95_us=%.0f p99_us=%.0f\n",latency:percentile(95),latency:percentile(99)))
end
