use crate::strategy::StrategyStatus;

pub fn render_dashboard(status: &StrategyStatus) -> String {
    // defaults
    let p_dec = status.custom.get("asset_precision")
        .and_then(|p| p.get("price_decimals"))
        .and_then(|v| v.as_u64())
        .unwrap_or(2);
        
    let s_dec = status.custom.get("asset_precision")
        .and_then(|p| p.get("sz_decimals"))
        .and_then(|v| v.as_u64())
        .unwrap_or(4);

    let grid_levels = status.custom.get("levels").and_then(|v| v.as_u64()).unwrap_or(0);
    
    // Attempt to get range
    let range = if let (Some(l), Some(u)) = (
        status.custom.get("lower_price").and_then(|v| v.as_f64()),
        status.custom.get("upper_price").and_then(|v| v.as_f64())
    ) {
        format!("{:.2} - {:.2}", l, u)
    } else {
        "N/A".to_string()
    };

    let pnl_color = if status.net_profit() >= 0.0 { "var(--buy)" } else { "var(--sell)" };

    format!(
            r##"<!DOCTYPE html>
<html>
<head>
    <title>{name} - Grid Terminal</title>
    <link href="https://fonts.googleapis.com/css2?family=Inter:wght@400;500;600;700&family=JetBrains+Mono:wght@400;500&display=swap" rel="stylesheet">
    <script src="https://unpkg.com/lightweight-charts@4.0.1/dist/lightweight-charts.standalone.production.js"></script>
    <style>

        :root {{
            --bg-dark: #0d0d12;
            --bg-panel: #16161f;
            --bg-hover: #1e1e2d;
            --border: #2a2a3a;
            --text-primary: #e6e6e6;
            --text-secondary: #9494a8;
            --brand: #00c2ff;
            --buy: #00c2a2;
            --buy-bg: rgba(0, 194, 162, 0.15);
            --sell: #ff3b69;
            --sell-bg: rgba(255, 59, 105, 0.15);
        }}

        * {{ box-sizing: border-box; }}

        body {{
            background: var(--bg-dark);
            color: var(--text-primary);
            font-family: 'Inter', sans-serif;
            margin: 0;
            height: 100vh;
            display: grid;
            grid-template-rows: 50px 1fr 350px; /* Header, Main, Bottom */
            overflow: hidden;
        }}

        /* --- Header --- */
        .app-header {{
            background: var(--bg-panel);
            border-bottom: 1px solid var(--border);
            display: flex;
            align-items: center;
            padding: 0 20px;
            justify-content: space-between;
        }}
        
        .brand {{
            font-weight: 700;
            font-size: 14px;
            display: flex;
            align-items: center;
            gap: 10px;
        }}
        .brand span {{ color: var(--brand); }}
        
        .market-stat {{
            display: flex;
            gap: 20px;
            font-size: 12px;
            font-family: 'JetBrains Mono', monospace;
        }}
        .stat-item {{ display: flex; flex-direction: column; }}
        .stat-label {{ color: var(--text-secondary); font-size: 10px; }}
        .stat-val {{ font-weight: 600; }}

        /* --- Main Area (Chart + Sidebar) --- */
        .main-container {{
            display: grid;
            grid-template-columns: 1fr 500px; /* Chart area, Side Panel */
            overflow: hidden;
        }}

        /* Center / Chart Area */
        .chart-area {{
            border-right: 1px solid var(--border);
            display: flex;
            flex-direction: column;
            padding: 20px;
            position: relative;
        }}
        
        #chartContainer {{
            flex: 1;
            width: 100%;
            height: 100%;
            border: 1px solid var(--border);
            border-radius: 4px;
            overflow: hidden;
            position: relative;
        }}

        /* Bot Info Widget (Floating or embedded in Chart area) */
        .bot-stats {{
            display: grid;
            grid-template-columns: repeat(4, 1fr);
            gap: 10px;
            margin-bottom: 20px;
        }}
        
        .card {{
            background: var(--bg-panel);
            border: 1px solid var(--border);
            padding: 12px;
            border-radius: 6px;
        }}
        .card-title {{ font-size: 11px; color: var(--text-secondary); margin-bottom: 4px; }}
        .card-value {{ font-size: 16px; font-weight: 600; font-family: 'JetBrains Mono', monospace; }}

        /* --- Side Panel (CLOB) --- */
        .side-panel {{
            background: var(--bg-panel);
            display: flex;
            flex-direction: column;
            overflow: hidden;
        }}

        /* Tabs */
        .tabs {{
            display: flex;
            border-bottom: 1px solid var(--border);
        }}
        .tab {{
            flex: 1;
            text-align: center;
            padding: 10px;
            font-size: 12px;
            color: var(--text-secondary);
            cursor: pointer;
            border-bottom: 2px solid transparent;
        }}
        .tab:hover {{ color: var(--text-primary); background: var(--bg-hover); }}
        .tab.active {{ color: var(--text-primary); border-bottom-color: var(--brand); }}

        /* Tab Content */
        .tab-content {{ flex: 1; display: none; flex-direction: column; overflow: hidden; }}
        .tab-content.active {{ display: flex; }}

        /* Reuse CLOB styles with tweaked colors */
        .clob-header {{
            display: grid;
            grid-template-columns: 40px 1fr 1fr 1fr;
            padding: 8px 12px;
            font-size: 10px;
            color: var(--text-secondary);
            font-family: 'JetBrains Mono', monospace;
            border-bottom: 1px solid var(--border);
        }}
        .row {{
            display: grid;
            grid-template-columns: 40px 1fr 1fr 1fr;
            padding: 3px 12px;
            font-size: 11px;
            font-family: 'JetBrains Mono', monospace;
            cursor: default;
        }}
        .row:hover {{ background: var(--bg-hover); }}
        .ask-price {{ color: var(--sell); }}
        .bid-price {{ color: var(--buy); }}
        .dist {{ color: var(--text-secondary); }}
        .col.right {{ text-align: right; }}
        .lvl-idx {{ color: var(--text-secondary); opacity: 0.5; }}

        .spread-row {{
            padding: 6px;
            text-align: center;
            font-size: 11px;
            font-family: 'JetBrains Mono', monospace;
            border-top: 1px solid var(--border);
            border-bottom: 1px solid var(--border);
            background: rgba(255,255,255,0.02);
            color: var(--text-secondary);
        }}
        
        .book-scroll-area {{
            flex: 1;
            overflow-y: auto;
            scrollbar-width: thin;
        }}

        /* --- Bottom Panel (Trades) --- */
        .bottom-panel {{
            border-top: 1px solid var(--border);
            background: var(--bg-panel);
            display: flex;
            flex-direction: column;
            overflow: hidden;
        }}
        
        .panel-header {{
            padding: 8px 20px;
            font-size: 12px;
            font-weight: 600;
            border-bottom: 1px solid var(--border);
            display: flex;
            gap: 20px;
        }}
        .panel-tab {{ cursor: pointer; color: var(--text-secondary); }}
        .panel-tab.active {{ color: var(--brand); }}

        .trades-table {{
            width: 100%;
            border-collapse: collapse;
            font-size: 12px;
            font-family: 'JetBrains Mono', monospace;
        }}
        .trades-table th {{
            text-align: left;
            padding: 8px 20px;
            color: var(--text-secondary);
            font-weight: normal;
            position: sticky;
            top: 0;
            background: var(--bg-panel);
            border-bottom: 1px solid var(--border);
        }}
        .trades-table td {{
            padding: 6px 20px;
            border-bottom: 1px solid var(--border);
        }}
        .trade-buy {{ color: var(--buy); }}
        .trade-sell {{ color: var(--sell); }}

    </style>
</head>
<body>
    <!-- 1. Header -->
    <header class="app-header">
        <div class="brand">
            <span>HELIX</span> // Grid
        </div>
        <div class="market-stat">
            <div class="stat-item">
                <span class="stat-label">ASSET</span>
                <span class="stat-val">{asset}</span>
            </div>
            <div class="stat-item">
                <span class="stat-label">RANGE</span>
                <span class="stat-val">{range}</span>
            </div>
             <div class="stat-item">
                <span class="stat-label">LEVELS</span>
                <span class="stat-val">{levels}</span>
            </div>
        </div>
    </header>

    <!-- 2. Main Area (Chart + Sidebar) -->
    <div class="main-container">
        <!-- Center Info -->
        <div class="chart-area">
            <!-- Bot Stats Widgets -->
            <div class="bot-stats">
                <div class="card">
                    <div class="card-title">Realized PnL</div>
                    <div class="card-value" id="realizedPnl" style="color: {pnl_color}">${pnl:.2}</div>
                </div>
                 <div class="card">
                    <div class="card-title">Total Fees</div>
                    <div class="card-value" id="totalFees">--</div>
                </div>
                <div class="card">
                    <div class="card-title">Position</div>
                    <div class="card-value" id="posVal">{pos:.4}</div>
                </div>
                 <div class="card">
                    <div class="card-title">Uptime</div>
                    <div class="card-value" id="uptime">--</div>
                </div>
            </div>

            <!-- Chart Container -->
            <div id="chartContainer"></div>
        </div>

        <!-- Right Sidebar (CLOB) -->
        <div class="side-panel">
            <div class="tabs">
                <div class="tab active" onclick="switchTab('book')">Order Book</div>
                <div class="tab" onclick="switchTab('recent')">Recent</div>
            </div>

            <!-- CLOB Tab -->
            <div id="tab-book" class="tab-content active">
                 <div class="clob-header">
                    <div class="col">Lvl</div>
                    <div class="col right">Price</div>
                    <div class="col right">Dist%</div>
                    <div class="col right">Size</div>
                </div>
                 <div class="book-scroll-area">
                    <div id="bookContainer" class="book-container">
                         <div style="padding: 20px; text-align: center; color: var(--text-secondary)">Loading...</div>
                    </div>
                 </div>
            </div>

            <!-- Recent Trades List (Sidebar version) -->
            <div id="tab-recent" class="tab-content">
                 <div style="padding: 10px; color: var(--text-secondary); font-size: 11px; text-align: center;">Last 50 Executions</div>
                 <div class="book-scroll-area">
                     <table class="trades-table" id="sidebarTrades">
                         <!-- Sidebar simplified trades -->
                     </table>
                 </div>
            </div>
        </div>
    </div>

    <!-- 3. Bottom Panel (Roundtrips/History) -->
    <div class="bottom-panel">
        <div class="panel-header">
            <div class="panel-tab active">Execution History</div>
            <div class="panel-tab">Roundtrip PnL (Alpha)</div>
        </div>
        <div style="flex: 1; overflow: auto; padding-bottom: 20px;">
             <table class="trades-table">
                <thead>
                    <tr>
                        <th>Time</th>
                        <th>Side</th>
                        <th>Price</th>
                        <th>Size</th>
                        <th>Value</th>
                    </tr>
                </thead>
                <tbody id="mainTradesBody">
                    <!-- Full history injected here -->
                </tbody>
            </table>
        </div>
    </div>
    
    <script>
        // Init with safe defaults
        let P_DEC = {p_dec};
        let S_DEC = {s_dec};
        let firstLoad = true;
        
        let chart;
        let candleSeries;
        let loadedCandles = false;
        let lastCandleFetchTime = 0;
        let lastCandleData = null; // Track the last candle for live updates

        function switchTab(tabName) {{
            // Sidebar tabs
            document.querySelectorAll('.side-panel .tab').forEach(t => t.classList.remove('active'));
            document.querySelectorAll('.side-panel .tab-content').forEach(c => c.classList.remove('active'));
            
            if (tabName === 'book') {{
                document.querySelector('.side-panel .tab:nth-child(1)').classList.add('active');
                document.getElementById('tab-book').classList.add('active');
            }} else {{
                document.querySelector('.side-panel .tab:nth-child(2)').classList.add('active');
                document.getElementById('tab-recent').classList.add('active');
            }}
        }}

        async function updateDashboard() {{
            try {{
                const res = await fetch('/api/status');
                const data = await res.json();
                
                // Update Precision
                if (data.custom.asset_precision) {{
                    P_DEC = data.custom.asset_precision.price_decimals;
                    S_DEC = data.custom.asset_precision.size_decimals;
                }}
                
                // Initialize Chart if needed
                if (!chart && data.asset) {{
                    console.log("Initializing chart for", data.asset);
                    const container = document.getElementById('chartContainer');
                    chart = LightweightCharts.createChart(container, {{
                        layout: {{ 
                            background: {{ type: 'solid', color: '#16161f' }}, 
                            textColor: '#9494a8', 
                        }},
                        grid: {{
                            vertLines: {{ color: '#2a2a3a' }},
                            horzLines: {{ color: '#2a2a3a' }},
                        }},
                        timeScale: {{
                            timeVisible: true,
                            secondsVisible: false,
                        }},
                        crosshair: {{
                            mode: LightweightCharts.CrosshairMode.Normal,
                        }},
                    }});
                    
                    try {{
                        candleSeries = chart.addCandlestickSeries({{
                            upColor: '#00c2a2',
                            downColor: '#ff3b69',
                            borderVisible: false,
                            wickUpColor: '#00c2a2',
                            wickDownColor: '#ff3b69',
                            lastValueVisible: true,
                            priceLineVisible: true,
                            priceLineColor: '#00c2ff',
                            priceLineWidth: 2,
                            priceLineStyle: 0,
                        }});
                    }} catch (e) {{
                        console.error("Error adding series. Chart object:", chart);
                        throw e;
                    }}
                    
                    // Handle Resize
                    new ResizeObserver(entries => {{
                        if (entries.length === 0 || entries[0].target !== container) {{ return; }}
                        const newRect = entries[0].contentRect;
                        chart.applyOptions({{ width: newRect.width, height: newRect.height }});
                    }}).observe(container);
                }}
                
                // Fetch Candles (initial + every 10 minutes)
                const now = Date.now();
                const shouldFetchCandles = !loadedCandles || (now - lastCandleFetchTime > 10 * 60 * 1000);
                
                if (shouldFetchCandles && data.asset && chart) {{
                    try {{
                        const coin = data.asset.split('/')[0];
                        const start = now - (24 * 60 * 60 * 1000); // 1 day
                        
                        const url = `/api/candles?coin=${{encodeURIComponent(coin)}}&interval=15m&start=${{start}}&end=${{now}}`;
                        
                        const cRes = await fetch(url);
                        if (!cRes.ok) {{ throw new Error("HTTP " + cRes.status); }}
                        const candles = await cRes.json();
                        
                        if (candles.error) {{
                             console.error("API Error:", candles.error);
                             loadedCandles = true;
                        }} else if (Array.isArray(candles)) {{
                            if (candles.length > 0) {{
                                const uniqueData = new Map();
                                candles.forEach(c => {{
                                    const t = c.t / 1000;
                                    if (!uniqueData.has(t)) {{
                                        uniqueData.set(t, {{
                                            time: t,
                                            open: parseFloat(c.o),
                                            high: parseFloat(c.h),
                                            low: parseFloat(c.l),
                                            close: parseFloat(c.c),
                                        }});
                                    }}
                                }});
                                
                                const chartData = Array.from(uniqueData.values()).sort((a,b) => a.time - b.time);
                                
                                try {{
                                    candleSeries.setData(chartData);
                                    chart.timeScale().fitContent();
                                    loadedCandles = true;
                                    lastCandleFetchTime = now;
                                    
                                    // Store last candle for live updates
                                    if (chartData.length > 0) {{
                                        lastCandleData = {{ ...chartData[chartData.length - 1] }};
                                    }}
                                }} catch (chartErr) {{
                                    console.error("Chart setData error:", chartErr);
                                    loadedCandles = true;
                                }}
                            }} else {{
                                console.warn("No candles returned for " + coin);
                                loadedCandles = true;
                            }}
                        }}
                    }} catch(e) {{
                        console.error("Candle fetch error:", e);
                    }}
                }}
                
                // Update last candle with current price (live updates)
                if (loadedCandles && lastCandleData) {{
                    let currentPrice = null;
                    
                    // Try to get current price from data
                    if (data.custom && data.custom.current_price && data.custom.current_price > 0) {{
                        currentPrice = data.custom.current_price;
                    }}
                    // Fallback: calculate mid-price from order book
                    else if (data.custom && data.custom.book) {{
                        const book = data.custom.book;
                        if (book.asks && book.asks.length > 0 && book.bids && book.bids.length > 0) {{
                            const bestAsk = book.asks[book.asks.length - 1].price;
                            const bestBid = book.bids[0].price;
                            currentPrice = (bestAsk + bestBid) / 2;
                        }}
                    }}
                    
                    if (currentPrice && currentPrice > 0) {{
                        const updatedCandle = {{
                            ...lastCandleData,
                            high: Math.max(lastCandleData.high, currentPrice),
                            low: Math.min(lastCandleData.low, currentPrice),
                            close: currentPrice,
                        }};
                        
                        try {{
                            candleSeries.update(updatedCandle);
                        }} catch(e) {{
                            console.error("Candle update error:", e);
                        }}
                    }}
                }}
                
                // --- Draw Grid Level Lines ---
                if (candleSeries && data.custom && data.custom.book) {{
                    // Clear existing price lines (if any)
                    if (!window.gridPriceLines) {{
                        window.gridPriceLines = [];
                    }}
                    
                    // Remove old lines
                    window.gridPriceLines.forEach(line => {{
                        try {{ candleSeries.removePriceLine(line); }} catch(e) {{}}
                    }});
                    window.gridPriceLines = [];
                    
                    const book = data.custom.book;
                    
                    // Draw Buy levels (green)
                    if (book.bids && Array.isArray(book.bids)) {{
                        book.bids.forEach(bid => {{
                            const line = candleSeries.createPriceLine({{
                                price: bid.price,
                                color: '#00c2a2',
                                lineWidth: 1,
                                lineStyle: 2, // Dashed
                                axisLabelVisible: false,
                                title: '',
                            }});
                            window.gridPriceLines.push(line);
                        }});
                    }}
                    
                    // Draw Sell levels (red)
                    if (book.asks && Array.isArray(book.asks)) {{
                        book.asks.forEach(ask => {{
                            const line = candleSeries.createPriceLine({{
                                price: ask.price,
                                color: '#ff3b69',
                                lineWidth: 1,
                                lineStyle: 2, // Dashed
                                axisLabelVisible: false,
                                title: '',
                            }});
                            window.gridPriceLines.push(line);
                        }});
                    }}
                }}
                
                // --- Current Price Line (Blue) ---
                if (candleSeries && data.custom && data.custom.book) {{
                    const book = data.custom.book;
                    
                    if (book.asks && book.asks.length > 0 && book.bids && book.bids.length > 0) {{
                        const bestAsk = book.asks[book.asks.length - 1].price;
                        const bestBid = book.bids[0].price;
                        const midPrice = (bestAsk + bestBid) / 2;
                        
                        // Remove old current price line if exists
                        if (window.currentPriceLine) {{
                            try {{ 
                                candleSeries.removePriceLine(window.currentPriceLine); 
                            }} catch(e) {{}}
                        }}
                        
                        // Create blue current price line
                        try {{
                            window.currentPriceLine = candleSeries.createPriceLine({{
                                price: midPrice,
                                color: '#00c2ff',
                                lineWidth: 1,
                                lineStyle: 0,
                                axisLabelVisible: true,
                                title: 'Px',
                            }});
                        }} catch(e) {{}}
                    }}
                }}


                // --- 1. Update Header / Bot Stats ---
                const pnl = data.realized_pnl - data.total_fees;
                const pnlEl = document.getElementById('realizedPnl');
                pnlEl.innerText = '$' + pnl.toFixed(2);
                pnlEl.style.color = pnl >= 0 ? 'var(--buy)' : 'var(--sell)';
                
                document.getElementById('totalFees').innerText = '$' + data.total_fees.toFixed(2);
                document.getElementById('posVal').innerText = data.position.toFixed(S_DEC);

                // --- 2. Render Order Book (Sidebar) ---
                const book = data.custom.book;
                const container = document.getElementById('bookContainer');
                
                if (!book) {{
                    container.innerHTML = '<div style="padding: 20px; text-align: center; color: var(--text-secondary)">No Order Book Data</div>';
                }} else {{
                    let html = '';
                    
                    // Asks
                    for (let i = 0; i < book.asks.length; i++) {{
                        const ask = book.asks[i];
                        html += `<div class="row">
                            <div class="col lvl-idx">${{ask.level_idx}}</div>
                            <div class="col right ask-price">${{ask.price.toFixed(P_DEC)}}</div>
                            <div class="col right dist">${{ask.dist.toFixed(2)}}%</div>
                            <div class="col right">${{ask.size.toFixed(S_DEC)}}</div>
                        </div>`;
                    }}

                    // Spread
                    if (book.asks.length > 0 && book.bids.length > 0) {{
                        const bestAsk = book.asks[book.asks.length - 1].price;
                        const bestBid = book.bids[0].price;
                        const spread = bestAsk - bestBid;
                        const spreadPct = (spread / bestAsk) * 100;
                        
                        let midPrice = (bestAsk + bestBid) / 2;
                        if (data.custom.current_price && data.custom.current_price > 0) {{
                            midPrice = data.custom.current_price;
                        }}
                        
                        html += `<div class="spread-row">
                            Spread: ${{spread.toFixed(P_DEC)}} (${{spreadPct.toFixed(3)}}%) 
                            <span style="color: var(--text-primary); margin-left: 8px">Px: ${{midPrice.toFixed(P_DEC)}}</span>
                        </div>`;
                        
                        // Update Chart Realtime Price Line (Optional)
                        if(candleSeries && !loadedCandles) {{ // Only if candles not loaded or just to show curent price
                           // LWC doesn't autoupdate candles, but we can update the last candle
                        }}
                        
                    }} else {{
                        html += `<div class="spread-row">No Active Spread</div>`;
                    }}

                    // Bids
                    for (const bid of book.bids) {{
                        html += `<div class="row">
                            <div class="col lvl-idx">${{bid.level_idx}}</div>
                            <div class="col right bid-price">${{bid.price.toFixed(P_DEC)}}</div>
                            <div class="col right dist">${{bid.dist.toFixed(2)}}%</div>
                            <div class="col right">${{bid.size.toFixed(S_DEC)}}</div>
                        </div>`;
                    }}
                    
                    container.innerHTML = html;

                    if (firstLoad && book.asks.length > 0) {{
                         const rowHeight = 22; 
                         const askHeight = book.asks.length * rowHeight;
                         const viewHeight = container.parentElement.clientHeight;
                         const scrollPos = askHeight - (viewHeight / 2);
                         container.parentElement.scrollTop = scrollPos > 0 ? scrollPos : 0;
                         firstLoad = false;
                    }}
                }}
                
                // --- 3. Render Trades (Sidebar & Bottom Panel) ---
                const trades = data.custom.recent_trades || [];
                
                // Sidebar List (Simplified)
                let sidebarHtml = '';
                 if (trades.length === 0) {{
                    sidebarHtml = '<tr><td colspan="3" style="text-align:center; padding: 20px;">No trades</td></tr>';
                }} else {{
                    for (const trade of trades) {{
                         const sideColor = trade.side === 'Buy' ? 'var(--buy)' : 'var(--sell)';
                         sidebarHtml += `<tr>
                            <td style="text-align: left; color: ${{sideColor}}">${{trade.price.toFixed(P_DEC)}}</td>
                            <td style="text-align: right">${{trade.size.toFixed(S_DEC)}}</td>
                            <td style="text-align: right; color: var(--text-secondary)">${{new Date(trade.time * 1000).toLocaleTimeString([], {{hour:'2-digit', minute:'2-digit'}})}}</td>
                        </tr>`;
                    }}
                }}
                document.getElementById('sidebarTrades').innerHTML = sidebarHtml;

                // Main Bottom Table (Detailed)
                let mainHtml = '';
                if (trades.length === 0) {{
                    mainHtml = '<tr><td colspan="5" style="text-align:center; padding: 20px;">No executions yet</td></tr>';
                }} else {{
                    for (const trade of trades) {{
                        const sideClass = trade.side === 'Buy' ? 'trade-buy' : 'trade-sell';
                        const timeStr = new Date(trade.time * 1000).toLocaleString();
                        const val = trade.price * trade.size;
                        mainHtml += `<tr>
                            <td>${{timeStr}}</td>
                            <td class="${{sideClass}}">${{trade.side.toUpperCase()}}</td>
                            <td>${{trade.price.toFixed(P_DEC)}}</td>
                            <td>${{trade.size.toFixed(S_DEC)}}</td>
                            <td>${{val.toFixed(2)}}</td>
                        </tr>`;
                    }}
                }}
                document.getElementById('mainTradesBody').innerHTML = mainHtml;

            }} catch (e) {{
                console.error("Fetch error:", e);
            }}
        }}

        setInterval(updateDashboard, 1000);
        updateDashboard();
    </script>
</body>
</html>
        "##,
            name = status.name,
            asset = status.asset,
            levels = grid_levels,
            range = range,
            pnl_color = pnl_color,
            pnl = status.net_profit(),
            pos = status.position,
            p_dec = p_dec,
            s_dec = s_dec
    )
}
