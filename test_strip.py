import re

html = """
                    <div id="command">
                    <span class="dropdown">
                    <span style="word-spacing: 0px;">
                    <b class="caret" data-toggle="dropdown"></b>
                    <span class="command0 simplecommandstart" helpref="help-3"><a href="/explain/1/gnutrue">true(1)</a></span>
                    <ul class="dropdown-menu" role="menu" aria-labelledby="dropdownMenu">
                      
                      <li class="dropdown-header">other manpages</li>
                      <li><a tabindex="-1" rel="nofollow" href="/explain?cmd=rust-true.1%20%26%26%20%7B%20echo%20success%3B%20%7D%20%7C%7C%20%7B%20echo%20failed%3B%20%7D">rust-true(1)</a></li><li><a tabindex="-1" rel="nofollow" href="/explain?cmd=true.1posix%20%26%26%20%7B%20echo%20success%3B%20%7D%20%7C%7C%20%7B%20echo%20failed%3B%20%7D">true(1posix)</a></li>
                      
                      
                    </ul>
                    </span>
                </span> <span class="shell" helpref="help-0">&amp;&amp;</span> <span class="shell" helpref="help-1">{</span> <span class="dropdown">
                    <span style="word-spacing: 0px;">
                    <b class="caret" data-toggle="dropdown"></b>
                    <span class="command1 simplecommandstart" helpref="help-4"><a href="/explain/1plan9/echo">echo(1plan9)</a></span>
                    <ul class="dropdown-menu" role="menu" aria-labelledby="dropdownMenu">
                      
                      <li class="dropdown-header">other manpages</li>
                      <li><a tabindex="-1" rel="nofollow" href="/explain?cmd=true%20%26%26%20%7B%20rust-echo.1%20success%3B%20%7D%20%7C%7C%20%7B%20echo%20failed%3B%20%7D">rust-echo(1)</a></li><li><a tabindex="-1" rel="nofollow" href="/explain?cmd=true%20%26%26%20%7B%20gnuecho.1%20success%3B%20%7D%20%7C%7C%20%7B%20echo%20failed%3B%20%7D">gnuecho(1)</a></li><li><a tabindex="-1" rel="nofollow" href="/explain?cmd=true%20%26%26%20%7B%20echo.1posix%20success%3B%20%7D%20%7C%7C%20%7B%20echo%20failed%3B%20%7D">echo(1posix)</a></li>
                      
                      
                    </ul>
                    </span>
                </span> <span class="command1" helpref="help-5">success</span><span class="shell" helpref="help-2">;</span> <span class="shell" helpref="help-1">}</span> <span class="shell" helpref="help-0">||</span> <span class="shell" helpref="help-1">{</span> <span class="dropdown">
                    <span style="word-spacing: 0px;">
                    <b class="caret" data-toggle="dropdown"></b>
                    <span class="command2 simplecommandstart" helpref="help-4"><a href="/explain/1plan9/echo">echo(1plan9)</a></span>
                    <ul class="dropdown-menu" role="menu" aria-labelledby="dropdownMenu">
                      
                      <li class="dropdown-header">other manpages</li>
                      <li><a tabindex="-1" rel="nofollow" href="/explain?cmd=true%20%26%26%20%7B%20echo%20success%3B%20%7D%20%7C%7C%20%7B%20rust-echo.1%20failed%3B%20%7D">rust-echo(1)</a></li><li><a tabindex="-1" rel="nofollow" href="/explain?cmd=true%20%26%26%20%7B%20echo%20success%3B%20%7D%20%7C%7C%20%7B%20gnuecho.1%20failed%3B%20%7D">gnuecho(1)</a></li><li><a tabindex="-1" rel="nofollow" href="/explain?cmd=true%20%26%26%20%7B%20echo%20success%3B%20%7D%20%7C%7C%20%7B%20echo.1posix%20failed%3B%20%7D">echo(1posix)</a></li>
                      
                      
                    </ul>
                    </span>
                </span> <span class="command2" helpref="help-5">failed</span><span class="shell" helpref="help-2">;</span> <span class="shell" helpref="help-1">}</span>
                    </div>
"""
# Remove <ul class="dropdown-menu"...>...</ul>
html_no_ul = re.sub(r'<ul\s+class="dropdown-menu".*?</ul>', '', html, flags=re.DOTALL)
html_no_tags = re.sub(r'<[^>]+>', '', html_no_ul)
import html as pyhtml
print(pyhtml.unescape(html_no_tags).strip().replace('\n', '').replace('  ', ' '))
