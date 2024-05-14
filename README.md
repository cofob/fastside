# fastside

A smart redirecting gateway for various frontend services. Faster and compatible
alternative to farside.

Contents

- [fastside](#fastside)
  - [About](#about)
  - [Demo](#demo)
  - [How It Works](#how-it-works)
  - [To Do](#to-do)

## About

A redirecting service for FOSS alternative frontends.

[Fastside](https://fastside.link) provides links that automatically redirect to
working instances of privacy-oriented alternative frontends, such as Nitter,
Libreddit, etc. This allows for users to have more reliable access to the
available public instances for a particular service, while also helping to
distribute traffic more evenly across all instances and avoid performance
bottlenecks and rate-limiting.

## Demo

Fastside's links work with the following structure: `fastside.link/<service>/<path>`

For example:

<table>
    <tr>
        <td>Service</td>
        <td>Page</td>
        <td>Fastside Link</td>
    </tr>
    <tr>
        <td><a href="https://github.com/spikecodes/libreddit">Libreddit</a></td>
        <td>/r/popular</td>
        <td><a href="https://fastside.link/libreddit/r/popular">https://fastside.link/libreddit/r/popular</a></td>
    </tr>
    <tr>
        <td><a href="https://codeberg.org/teddit/teddit">Teddit</a></td>
        <td>/r/popular</td>
        <td><a href="https://fastside.link/teddit/r/popular">https://fastside.link/teddit/r/popular</a></td>
    </tr>
    <tr>
        <td><a href="https://github.com/iv-org/invidious">Invidious</a></td>
        <td>/watch?v=zLGDE2j_n5c</td>
        <td><a href="https://fastside.link/_/invidious/watch?v=zLGDE2j_n5c">https://fastside.link/_/invidious/watch?v=zLGDE2j_n5c</a></td>
    </tr>
    <tr>
        <td><a href="https://github.com/httpjamesm/AnonymousOverflow">AnonymousOverflow</a></td>
        <td>/questions/1732348/regex-match-open-tags-except-xhtml-self-contained-tags</td>
        <td><a href="https://fastside.link/@cached/anonymousoverflow#questions/1732348/regex-match-open-tags-except-xhtml-self-contained-tags">https://fastside.link/@cached/anonymousoverflow#questions/1732348/regex-match-open-tags-except-xhtml-self-contained-tags</a></td>
    </tr>
    <!-- more rows can be added as needed -->
</table>

<sup>Note: This table doesn't include all available services. For a complete list of supported frontends, see: https://github.com/cofob/fastside/blob/master/services.json</sup>

Additionally, Fastside includes a caching feature that makes redirects faster:
`/@cached/<service>#<path>`

## How It Works

The app runs with an internally scheduled cron task that queries all instances
for services defined in [services.json](./services.json) every 5 minutes. For
each instance, as long as the instance takes <2 seconds to respond and returns
a successful response code, the instance is added to a list of available
instances for that particular service. If not, it is discarded until the next
update period.

Fastside's routing is minimal, similar to Farside, but includes an additional `/@cached/<service>#<path>` endpoint,
which utilizes browser caching to achieve instant redirects without waiting for server responses.

## To Do

- [ ] GeoDB integration
