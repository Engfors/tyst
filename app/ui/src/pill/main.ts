import { mount } from "svelte";
import "../lib/base.css";
import Pill from "./Pill.svelte";

mount(Pill, { target: document.getElementById("app")! });
