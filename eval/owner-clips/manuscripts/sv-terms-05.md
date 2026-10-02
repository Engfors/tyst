# sv-terms-05 · Jouren i helgen

Kategori: sv-terms · Språk: sv · Mål: ca 105 s
Termer: Kubernetes, deploy, staging, Helm, GitHub, pipeline, state drift, Terraform, Vault, pull request, rollback, plus böjda former (containrar, deployade).

---
I: Du hade jour i helgen. Hände det något?
G: Ja, på lördagskvällen gick ett larm om att betaltjänsten svarade långsamt. Jag loggade in och såg att flera containrar hade startats om i Kubernetes de senaste tio minuterna.
I: Vad var orsaken?
G: Det visade sig att en deploy från fredagen hade fått fel minnesgräns. I staging hade det fungerat, men där är ju trafiken mycket lägre. Så jag gjorde en rollback med Helm, och efter ett par minuter var allt stabilt igen.
I: Hur kunde fel värde komma med?
G: Någon hade ändrat värdet direkt i klustret under en tidigare incident och sedan aldrig fört in det i koden i GitHub. När vår pipeline deployade den nya versionen skrevs den manuella ändringen över. Det är egentligen samma sorts state drift som vi har med Terraform, fast i Kubernetes.
I: Fick du hjälp av någon?
G: Jag behövde inloggningsuppgifter till databasen för att kontrollera att inga betalningar hade fastnat, och dem hämtade jag från Vault. Annars klarade jag det själv. Men på måndagen skapade jag en pull request som lägger in rätt minnesgräns i koden, så att det inte händer igen.
