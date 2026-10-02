# sv-terms-02 · Vault och hemligheter

Kategori: sv-terms · Språk: sv · Mål: ca 95 s
Termer: Vault, HashiCorp, Terraform, pipeline, container, Kubernetes, GitHub, pull request.

---
I: Ni har ju infört Vault det senaste året. Varför?
G: Förut låg lösenord och nycklar lite överallt. En del i konfigurationsfiler, en del som variabler i vår pipeline, och en del, ärligt talat, i någons anteckningar. Vi ville ha ett ställe för alla hemligheter, och HashiCorp Vault var det självklara valet eftersom vi redan kör Terraform.
I: Hur fungerar det i praktiken?
G: När en container startar loggar den in i Vault med sitt servicekonto i Kubernetes och får tillbaka kortlivade inloggningsuppgifter till databasen. De gäller i en timme och förnyas automatiskt. Så ingen människa behöver någonsin se lösenordet.
I: Var det svårt att införa?
G: Tekniskt sett inte särskilt. Det svåra var att få alla team att sluta lägga hemligheter i GitHub. Vi lade till en kontroll i varje pull request som stoppar bygget om den hittar något som ser ut som en nyckel. Det var inte populärt de första veckorna.
I: Och nu?
G: Nu tänker ingen på det längre. Det är så det ska vara. Vault är en av de där sakerna man bara märker när den inte fungerar.
