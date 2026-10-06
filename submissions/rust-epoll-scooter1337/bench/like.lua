local tokens,threads={},{}
errors=0
function setup(t) threads[#threads+1]=t end
function init(args)
 local f=assert(io.open('/bench/seed/tokens.json','r'))
 for t in f:read('*a'):gmatch('"token"%s*:%s*"([^"]+)"') do tokens[#tokens+1]=t end
 f:close();math.randomseed(42)
end
function request()
 return wrk.format('POST','/posts/500000/like',{Authorization='Bearer '..tokens[math.random(#tokens)],['Content-Type']='application/json'},'')
end
function response(status,headers,body)
 if (status~=201 and status~=200) or body:sub(1,13)~='{"liked":true' then errors=errors+1 end
end
function done(summary,latency,requests)
 local n=0;for _,t in ipairs(threads) do n=n+(t:get('errors') or 0) end
 io.write('HTTP semantic errors: '..n..'\n')
 io.write(string.format('LATENCY p95_us=%.0f p99_us=%.0f\n',latency:percentile(95),latency:percentile(99)))
end
