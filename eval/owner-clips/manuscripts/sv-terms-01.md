# sv-terms-01 · Terraform och state drift

Kategori: sv-terms · Språk: sv · Mål: ca 95 s
Termer: Terraform, GitHub, pull request, pipeline, staging, state drift, deploy (som "deploya").

---
I: Hur jobbar ni med infrastrukturen i dag?
G: Allt ligger i Terraform. Varje miljö har sin egen katalog, och koden ligger i GitHub. När någon vill ändra något skapar man en pull request, och sedan måste minst en kollega godkänna den innan den får mergas.
I: Och vad händer sedan?
G: Då startar en pipeline som kör en plan mot staging. Om planen ser rimlig ut körs apply automatiskt, och sedan väntar pipelinen på ett manuellt godkännande innan samma ändring går ut i produktion.
I: Har ni problem med state drift?
G: Ja, tyvärr. Det händer att någon ändrar en inställning direkt i konsolen när det brinner, och sedan glömmer att uppdatera Terraform. Då får vi state drift, och nästa plan vill plötsligt ta bort något som alla trodde skulle finnas kvar.
I: Hur hanterar ni det?
G: Vi har ett nattligt jobb i vår pipeline som kör en plan mot alla miljöer och larmar om den hittar state drift. Det är inte perfekt, men vi upptäcker det åtminstone inom ett dygn i stället för när någon försöker deploya nästa gång.
