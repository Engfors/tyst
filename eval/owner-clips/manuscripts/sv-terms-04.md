# sv-terms-04 · Kodgranskning i GitHub

Kategori: sv-terms · Språk: sv · Mål: ca 90 s
Termer: pull request (många gånger), GitHub, pipeline, Terraform, staging, deploy (som "deploya").

---
I: Hur ser er kodgranskning ut?
G: Allt går via pull requests i GitHub. Vi har en regel att en pull request inte får vara större än att man hinner granska den på en kvart. Blir den större ska den delas upp.
I: Varför just en kvart?
G: För att stora pull requests inte granskas på riktigt. Folk scrollar igenom, skriver ser bra ut och godkänner. Små ändringar får betydligt bättre feedback, och de går dessutom snabbare att deploya.
I: Använder ni några verktyg för att automatisera granskningen?
G: Ja, vi har en pipeline som körs på varje pull request. Den kör tester, kontrollerar formateringen och kör en plan om ändringen rör Terraform. Resultatet läggs upp som en kommentar direkt i GitHub, så att granskaren ser exakt vad som kommer att hända i staging.
I: Hur länge får en pull request ligga och vänta?
G: Målet är att någon ska ha tittat på den inom fyra timmar. Vi följer upp det varje vecka, och om det börjar dra iväg tar vi upp det på retrospektivet.
